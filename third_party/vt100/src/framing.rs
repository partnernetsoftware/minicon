//! Bounded transport framing driven by the authoritative terminal grammar.
use std::io::{self, Write};

/// Working chunk size for encoded producers and Unicode text lookahead.
///
/// This is separate from the unfinished-frame budget. The ASCII fast path can
/// publish an entire caller-bounded slice without lookahead or staging.
pub const TEXT_BATCH_BYTES: usize = 64 * 1024;

/// Failure while publishing a framed client stream.
#[derive(Debug)]
pub enum FrameError {
    /// The unfinished frame exceeded the caller's byte budget. Its retained
    /// bytes and input grammar were discarded; screen and callbacks survive.
    PendingLimit,
    /// The sink failed. A partially written sink must be retired by its owner.
    Io(io::Error),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PendingLimit => f.write_str("unfinished terminal frame exceeds budget"),
            Self::Io(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for FrameError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::PendingLimit => None,
            Self::Io(error) => Some(error),
        }
    }
}

impl From<io::Error> for FrameError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Retains incomplete VT/UTF-8 frames so an owner can interleave complete
/// native projections while the downstream decoder is at ground.
///
/// Use one framer per parser and sink. Route all client bytes through `write`;
/// do not directly process or reset that parser's input grammar while pending
/// bytes exist. Screen-only edits preserve this relationship. This type has
/// no OS status codes, native handles, threads or process-lifetime policy.
/// A rejected incomplete UTF-8 prefix is published as canonical U+FFFD before
/// the invalidating byte starts new input. Valid frames retain their wire bytes.
pub struct OutputFramer {
    pending: Vec<u8>,
    limit: usize,
}

impl OutputFramer {
    /// Choose the maximum retained frame size. A zero budget permits the
    /// ASCII fast path but rejects any byte requiring frame staging.
    #[must_use]
    pub fn new(limit: usize) -> Self {
        Self {
            pending: Vec::new(),
            limit,
        }
    }

    /// Accepted bytes still awaiting a complete grammar boundary.
    #[must_use]
    pub fn pending_bytes(&self) -> &[u8] {
        &self.pending
    }

    /// Publish complete frames and immediately executed controls. Commit each
    /// byte to the authoritative parser only after its required sink write.
    /// Earlier complete frames may have been published when an error occurs.
    ///
    /// # Errors
    /// Returns a bounded-frame rejection or the underlying sink failure.
    pub fn write<CB: crate::Callbacks>(
        &mut self,
        parser: &mut crate::Parser<CB>,
        bytes: &[u8],
        output: &mut impl Write,
    ) -> Result<(), FrameError> {
        self.write_mapped(parser, bytes, output, true, |parser, raw, delta, output| {
            output.write_all(raw)?;
            parser.process(delta);
            Ok(())
        })
    }

    /// Publish frames and visual runs through a transactional adapter. Raw bytes are the
    /// complete wire frame (canonical U+FFFD for a rejected UTF-8 prefix);
    /// delta contains bytes not yet semantically committed by the
    /// authoritative parser. Incomplete input advances grammar alone; a complete
    /// staged frame is replayed from ground at publication. The publisher must
    /// process/commit delta exactly
    /// once on success, preserving parser state on failure. Already executed
    /// C0 controls are excluded from a later retained frame.
    ///
    /// Set `ascii_controls` false for render adapters that need separate effect
    /// ordering for effectful C0 controls rather than one mixed ASCII batch.
    /// Visual CR/LF controls can still batch with printable ASCII.
    ///
    /// # Errors
    /// Returns a bounded-frame rejection or publisher failure.
    pub fn write_mapped<CB: crate::Callbacks, W: Write>(
        &mut self,
        parser: &mut crate::Parser<CB>,
        bytes: &[u8],
        output: &mut W,
        ascii_controls: bool,
        mut publish: impl FnMut(&mut crate::Parser<CB>, &[u8], &[u8], &mut W) -> io::Result<()>,
    ) -> Result<(), FrameError> {
        if bytes.is_empty() {
            return Ok(());
        }
        // Ordinary ASCII needs neither a copied parser state nor staging.
        if parser.at_boundary()
            && bytes.iter().all(|byte| {
                byte.is_ascii()
                    && *byte != 27
                    && (ascii_controls
                        || (32..127).contains(byte)
                        || matches!(*byte, b'\r' | b'\n'))
            })
        {
            publish(parser, bytes, bytes, output)?;
            return Ok(());
        }
        let mut remaining = bytes;
        while let Some((&byte, rest)) = remaining.split_first() {
            let length = self.text_prefix(parser, remaining, true);
            if length != 0 {
                publish(parser, &remaining[..length], &remaining[..length], output)?;
                remaining = &remaining[length..];
                continue;
            }
            let progress = parser.preview_byte(byte);
            if progress.invalidated_utf8 {
                // Publish the rejected prefix independently, keeping the peer
                // at ground before a new scalar, effect or native edit. Raw
                // malformed prefix bytes cannot provide that boundary alone.
                const REPLACEMENT: &[u8] = "\u{fffd}".as_bytes();
                parser.with_ground_input(|parser| {
                    publish(parser, REPLACEMENT, REPLACEMENT, output)
                })?;
                self.pending.clear();
                continue;
            }
            remaining = rest;
            if progress.executed_control && !progress.at_boundary {
                // Executed C0 controls do not belong to the retained grammar
                // payload. The peer is at ground and executes each exactly once.
                publish(parser, &[byte], &[byte], output)?;
                continue;
            }
            if self.pending.len() == self.limit {
                // No unfinished bytes have reached the consumer. Discard only
                // input grammar and reject this write, keeping screen state
                // available for subsequent client output.
                self.pending.clear();
                parser.reset_input();
                return Err(FrameError::PendingLimit);
            }
            self.pending.push(byte);
            if progress.at_boundary {
                if let Err(error) = parser.with_ground_input(|parser| {
                    publish(parser, &self.pending, &self.pending, output)
                }) {
                    self.pending.pop();
                    return Err(FrameError::Io(error));
                }
                self.pending.clear();
            } else {
                parser.advance_input_only(byte);
            }
        }
        Ok(())
    }

    /// Length of a bounded, complete UTF-8 text prefix at the current input
    /// boundary. Source screen, grammar, pending bytes and callbacks are not
    /// changed. Nonvisual controls and malformed/incomplete scalars stop it.
    /// `newlines` permits visual CR/LF. Each scalar retains the pending budget
    /// check even though a complete run needs no retained staging allocation.
    #[must_use]
    pub fn text_prefix<CB: crate::Callbacks>(
        &self,
        parser: &crate::Parser<CB>,
        bytes: &[u8],
        newlines: bool,
    ) -> usize {
        parser.text_prefix(bytes, self.limit, newlines)
    }

    /// Publish an accepted incomplete tail after the owner establishes that
    /// no further producer or native projection can follow. An I/O failure
    /// retains the pending bytes; retry is safe only if no bytes were written.
    ///
    /// # Errors
    /// Returns the underlying sink failure.
    pub fn finish(&mut self, output: &mut impl Write) -> io::Result<()> {
        output.write_all(&self.pending)?;
        self.pending.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{FrameError, OutputFramer};

    #[test]
    fn stream_boundary_utf8_recovery_is_transactional_and_preserves_decode_semantics() {
        #[derive(Default)]
        struct Bells(usize);
        impl crate::Callbacks for Bells {
            fn audible_bell(&mut self, _: &mut crate::Screen) {
                self.0 += 1;
            }
        }
        for prefix in [
            &b"\xe4"[..],
            &b"\xe4\xb8"[..],
            &b"\xef\xbf"[..],
            &b"\xf0\x9f\x98"[..],
            &b"\xe0"[..],
        ] {
            for suffix in [
                &b"\xe3\x81\x82"[..],
                &b"\x1b[31mZ"[..],
                &b"\x07Z"[..],
                &b"\xffZ"[..],
                &b"A"[..],
                &b"\x80Z"[..],
            ] {
                let mut parser = crate::Parser::new_with_callbacks(3, 16, 0, Bells::default());
                let mut direct = crate::Parser::new_with_callbacks(3, 16, 0, Bells::default());
                let mut framer = OutputFramer::new(64);
                let mut wire = Vec::new();
                for &byte in prefix.iter().chain(suffix) {
                    framer.write(&mut parser, &[byte], &mut wire).unwrap();
                    direct.process(&[byte]);
                    assert_eq!(
                        parser.screen().state_formatted(),
                        direct.screen().state_formatted()
                    );
                    assert_eq!(parser.callbacks().0, direct.callbacks().0);
                }
                let mut peer = crate::Parser::new_with_callbacks(3, 16, 0, Bells::default());
                peer.process(&wire);
                assert_eq!(
                    peer.screen().state_formatted(),
                    parser.screen().state_formatted()
                );
                assert_eq!(peer.callbacks().0, parser.callbacks().0);
                assert!(peer.at_boundary());
            }
        }
        let mut parser = crate::Parser::new(3, 16, 0);
        let mut framer = OutputFramer::new(3);
        let mut wire = Vec::new();
        framer.write(&mut parser, b"\xe4\xb8", &mut wire).unwrap();
        let before = parser.screen().state_formatted();
        assert!(framer.write(&mut parser, b"\xe3", &mut Reject).is_err());
        assert_eq!(framer.pending_bytes(), b"\xe4\xb8");
        assert!(!parser.at_boundary());
        assert_eq!(parser.screen().state_formatted(), before);
        // Retry a different byte: rollback must restore the exact old prefix.
        framer.write(&mut parser, b"\xad", &mut wire).unwrap();
        assert_eq!(wire, "中".as_bytes());
        assert_eq!(parser.screen().contents(), "中");
        framer
            .write(&mut parser, b"\xe4\xb8\xe3", &mut wire)
            .unwrap();
        assert_eq!(framer.pending_bytes(), b"\xe3");
        let before_tail = wire.len();
        framer.finish(&mut wire).unwrap();
        assert_eq!(&wire[before_tail..], b"\xe3");
        assert_eq!(parser.screen().contents(), "中\u{fffd}");
        let mut parser = crate::Parser::new(3, 16, 0);
        let mut framer = OutputFramer::new(2);
        // A two-byte pending budget deliberately rejects a three-byte scalar.
        // Recoverable rejection leaves no staged grammar or visual mutation.
        assert!(matches!(
            framer.write(&mut parser, b"\xe4\xb8\xad", &mut wire),
            Err(FrameError::PendingLimit)
        ));
        assert!(parser.at_boundary());
        assert!(framer.pending_bytes().is_empty());
        assert_eq!(parser.screen().contents(), "");
    }

    #[test]
    fn stream_boundary_unicode_runs_bound_publications_and_preserve_wire() {
        use std::io::Write as _;
        let text = "A中😀BCe\u{301}".repeat(512);
        let mut parser = crate::Parser::new(64, 160, 0);
        let mut framer = OutputFramer::new(64);
        let mut wire = Vec::new();
        let mut calls = 0;
        framer
            .write_mapped(
                &mut parser,
                text.as_bytes(),
                &mut wire,
                false,
                |parser, raw, delta, output| {
                    calls += 1;
                    output.write_all(raw)?;
                    parser.process(delta);
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(
            calls, 1,
            "ordinary Unicode must publish a text run, not individual scalars"
        );
        assert_eq!(wire, text.as_bytes());
        let mut direct = crate::Parser::new(64, 160, 0);
        direct.process(text.as_bytes());
        assert_eq!(
            parser.screen().state_formatted(),
            direct.screen().state_formatted()
        );
        assert!(framer.pending_bytes().is_empty());
        assert!(parser.at_boundary());
        assert_eq!(framer.text_prefix(&parser, "中\x07X".as_bytes(), true), 3);
        assert_eq!(framer.text_prefix(&parser, b"\x1b[31mZ", true), 0);
        assert_eq!(
            OutputFramer::new(2).text_prefix(&parser, "中".as_bytes(), true),
            0
        );
        let large = text.repeat(21);
        let mut chunks = 0;
        let mut large_wire = Vec::new();
        framer
            .write_mapped(
                &mut parser,
                large.as_bytes(),
                &mut large_wire,
                false,
                |parser, raw, delta, output| {
                    assert!(raw.len() <= 65536);
                    assert_eq!(raw, delta);
                    chunks += 1;
                    output.write_all(raw)?;
                    parser.process(delta);
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(chunks, 3);
        assert_eq!(large_wire, large.as_bytes());
    }

    struct Reject;

    impl std::io::Write for Reject {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("rejected before any bytes"))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn stream_boundary_framing_bounds_recovery_and_sink_commit() {
        let mut parser = crate::Parser::new(3, 8, 0);
        let mut framer = OutputFramer::new(4);
        let mut wire = Vec::new();
        framer.write(&mut parser, b"A\x1b[", &mut wire).unwrap();
        framer.write(&mut parser, b"\n3", &mut wire).unwrap();
        assert_eq!(wire, b"A\n");
        assert_eq!(parser.screen().cursor_position(), (1, 1));
        assert_eq!(framer.pending_bytes(), b"\x1b[3");
        assert!(matches!(
            framer.write(&mut parser, b"m", &mut Reject),
            Err(FrameError::Io(_))
        ));
        assert!(!parser.at_boundary());
        assert_eq!(framer.pending_bytes(), b"\x1b[3");
        framer.write(&mut parser, b"mB", &mut wire).unwrap();
        assert_eq!(wire, b"A\n\x1b[3mB");
        assert_eq!(parser.screen().cell(1, 1).unwrap().contents(), "B");
        assert!(matches!(
            framer.write(&mut parser, b"\x1b[123", &mut wire),
            Err(FrameError::PendingLimit)
        ));
        assert!(parser.at_boundary());
        assert!(framer.pending_bytes().is_empty());
        framer.write(&mut parser, b"C\xe4\xb8", &mut wire).unwrap();
        assert!(framer.finish(&mut Reject).is_err());
        assert_eq!(framer.pending_bytes(), &[0xe4, 0xb8]);
        framer.finish(&mut wire).unwrap();
        assert!(framer.pending_bytes().is_empty());
        assert!(wire.ends_with(b"BC\xe4\xb8"));
    }
}
