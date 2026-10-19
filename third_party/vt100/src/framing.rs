//! Bounded transport framing driven by the authoritative terminal grammar.
use std::io::{self, Write};

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
        // Ordinary ASCII needs neither a copied parser state nor staging.
        if parser.at_boundary() && bytes.iter().all(|byte| byte.is_ascii() && *byte != 27) {
            output.write_all(bytes)?;
            parser.process(bytes);
            return Ok(());
        }
        for &byte in bytes {
            let progress = parser.preview_byte(byte);
            if progress.executed_control && !progress.at_boundary {
                // Executed C0 controls do not belong to the retained grammar
                // payload. The peer is at ground and executes each exactly once.
                output.write_all(&[byte])?;
                parser.process(&[byte]);
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
                if let Err(error) = output.write_all(&self.pending) {
                    self.pending.pop();
                    return Err(FrameError::Io(error));
                }
                parser.process(&[byte]);
                self.pending.clear();
            } else {
                parser.process(&[byte]);
            }
        }
        Ok(())
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
