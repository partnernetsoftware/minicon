//! Common response grammar and finite CPR/key disambiguation; no shadow screen.
use std::{collections::VecDeque, fmt, time::Duration};
const MAX_CPR: usize = 64;

/// The caller must retire its transport after a query-inventory overflow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResponseTrackingError;
impl fmt::Display for ResponseTrackingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("terminal cursor-query tracking capacity exceeded")
    }
}
impl std::error::Error for ResponseTrackingError {}

struct Queries {
    deadlines: VecDeque<Duration>,
    now: Duration,
    timeout: Duration,
    failed: bool,
}
impl vte::Perform for Queries {
    fn csi_dispatch(
        &mut self,
        params: &vte::Params,
        intermediates: &[u8],
        ignore: bool,
        action: char,
    ) {
        let mut params = params.iter();
        if ignore
            || !intermediates.is_empty()
            || action != 'n'
            || params.next() != Some(&[6][..])
            || params.next().is_some()
            || self.failed
            || self.timeout.is_zero()
        {
            return;
        }
        if self.deadlines.len() == MAX_CPR {
            self.deadlines.clear();
            self.failed = true;
        } else {
            self.deadlines
                .push_back(self.now.saturating_add(self.timeout));
        }
    }
}

/// Observe accepted outgoing bytes using the owned lexer. DA1, DSR and DECRPM
/// reports have unambiguous grammar. CPR is admitted only while an observed
/// query awaits one reply; identical F3 keys during that window stay ambiguous.
///
/// Supply a monotonic elapsed clock. At most 64 CPR expectations survive until
/// their individual timeout. Overflow permanently fails the tracker; retire
/// the transport. A zero timeout disables ambiguous response admission.
pub struct TerminalResponses {
    parser: vte::Parser,
    queries: Queries,
}
impl TerminalResponses {
    #[must_use]
    pub fn new(timeout: Duration) -> Self {
        Self {
            parser: vte::Parser::new(),
            queries: Queries {
                deadlines: VecDeque::new(),
                now: Duration::ZERO,
                timeout,
                failed: false,
            },
        }
    }
    /// Observe bytes in accepted outgoing stream order. On error the observer
    /// cannot be reused, including when the caller rejects the current write.
    pub fn observe_output(
        &mut self,
        bytes: &[u8],
        now: Duration,
    ) -> Result<(), ResponseTrackingError> {
        self.expire(now);
        self.parser.advance(&mut self.queries, bytes);
        if self.is_failed() {
            Err(ResponseTrackingError)
        } else {
            Ok(())
        }
    }
    #[must_use]
    pub fn is_failed(&self) -> bool {
        self.queries.failed
    }
    /// Inspect one complete report without spending its CPR credit.
    pub fn reply_length(&mut self, bytes: &[u8], now: Duration) -> Option<usize> {
        self.expire(now);
        if self.is_failed() {
            return None;
        }
        let (length, cpr) = report(bytes)?;
        (!cpr || !self.queries.deadlines.is_empty()).then_some(length)
    }
    /// Spend a CPR credit only after the input sink accepts the span. Failed
    /// or backpressured input must leave the credit available for retry.
    pub fn accept_reply(&mut self, bytes: &[u8], now: Duration) {
        self.expire(now);
        if let Some((_, true)) = report(bytes) {
            self.queries.deadlines.pop_front();
        }
    }
    fn expire(&mut self, now: Duration) {
        self.queries.now = now;
        while self
            .queries
            .deadlines
            .front()
            .is_some_and(|deadline| *deadline <= now)
        {
            self.queries.deadlines.pop_front();
        }
    }
}
fn number(bytes: &[u8], maximum: u32) -> Option<u32> {
    if bytes.is_empty() || bytes.len() > 5 || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let value = std::str::from_utf8(bytes).ok()?.parse::<u32>().ok()?;
    (value <= maximum).then_some(value)
}
fn report(bytes: &[u8]) -> Option<(usize, bool)> {
    let tail = bytes.strip_prefix(b"\x1b[")?;
    let end = tail
        .iter()
        .take(64)
        .position(|byte| (0x40..=0x7e).contains(byte))?;
    let body = &tail[..end];
    let valid = match tail[end] {
        b'n' => body == b"0",
        b'c' => body
            .strip_prefix(b"?")
            .is_some_and(|p| p.split(|b| *b == b';').all(|p| number(p, 65535).is_some())),
        b'y' => {
            let body = body.strip_prefix(b"?")?.strip_suffix(b"$")?;
            let separator = body.iter().position(|b| *b == b';')?;
            number(&body[..separator], 65535).is_some()
                && matches!(&body[separator + 1..], [b'0'..=b'4'])
        }
        b'R' => {
            let separator = body.iter().position(|b| *b == b';')?;
            number(&body[..separator], 65536).is_some_and(|n| n != 0)
                && number(&body[separator + 1..], 65536).is_some_and(|n| n != 0)
        }
        _ => false,
    };
    valid.then_some((2 + end + 1, tail[end] == b'R'))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn terminal_responses_track_split_queries_acknowledgement_expiry_and_quota() {
        let mut responses = TerminalResponses::new(Duration::from_secs(2));
        let zero = Duration::ZERO;
        for wire in [b"\x1b[?1;2c".as_slice(), b"\x1b[0n", b"\x1b[?2004;1$y"] {
            for end in 0..wire.len() {
                assert_eq!(responses.reply_length(&wire[..end], zero), None);
            }
            let mut joined = wire.to_vec();
            joined.extend_from_slice(b"\x1b[Aq");
            assert_eq!(responses.reply_length(&joined, zero), Some(wire.len()));
        }
        for split in 0..=4 {
            let mut r = TerminalResponses::new(Duration::from_secs(2));
            r.observe_output(&b"\x1b[6n"[..split], zero).unwrap();
            assert_eq!(
                r.reply_length(b"\x1b[1;2R", zero),
                if split == 4 { Some(6) } else { None }
            );
            r.observe_output(&b"\x1b[6n"[split..], zero).unwrap();
            assert_eq!(r.reply_length(b"\x1b[1;2R", zero), Some(6));
            assert_eq!(r.reply_length(b"\x1b[1;2R", zero), Some(6));
            r.accept_reply(b"\x1b[1;2R", zero);
            assert_eq!(r.reply_length(b"\x1b[1;2R", zero), None);
        }
        responses.observe_output(b"\x1b[6n", zero).unwrap();
        responses
            .observe_output(b"\x1b[6n", Duration::from_secs(1))
            .unwrap();
        assert_eq!(
            responses.reply_length(b"\x1b[65536;65536R", Duration::from_secs(2)),
            Some(14)
        );
        responses.accept_reply(b"\x1b[65536;65536R", Duration::from_secs(2));
        assert_eq!(
            responses.reply_length(b"\x1b[1;2R", Duration::from_secs(2)),
            None
        );
        responses
            .observe_output(b"\x1b[6n", Duration::from_secs(3))
            .unwrap();
        assert_eq!(
            responses.reply_length(b"\x1b[1;2R", Duration::from_secs(5)),
            None
        );
        for wire in [
            b"\x1b[0;2R".as_slice(),
            b"\x1b[1;65537R",
            b"\x1b[1;2;3R",
            b"\x1b[?1;;2c",
            b"\x1b[1n",
            b"\x1b[?1;5$y",
        ] {
            assert_eq!(responses.reply_length(wire, Duration::from_secs(5)), None);
        }
        responses
            .observe_output(
                b"\x1b[?6n\x1b[6;5n\x1b[6$n\x1b[6\x18n",
                Duration::from_secs(5),
            )
            .unwrap();
        assert_eq!(
            responses.reply_length(b"\x1b[1;2R", Duration::from_secs(5)),
            None
        );
        responses
            .observe_output(b"\x1b[6n".repeat(64).as_slice(), Duration::from_secs(5))
            .unwrap();
        assert_eq!(
            responses.observe_output(b"\x1b[6n", Duration::from_secs(5)),
            Err(ResponseTrackingError)
        );
        assert!(responses.is_failed());
        assert_eq!(
            responses.reply_length(b"\x1b[1;2R", Duration::from_secs(5)),
            None
        );
        assert_eq!(
            responses.observe_output(b"ok", Duration::from_secs(99)),
            Err(ResponseTrackingError)
        );
        let mut disabled = TerminalResponses::new(Duration::ZERO);
        disabled
            .observe_output(&b"\x1b[6n".repeat(1000), Duration::ZERO)
            .unwrap();
        assert_eq!(disabled.reply_length(b"\x1b[1;2R", Duration::ZERO), None);
        assert!(!disabled.is_failed());
    }
    #[test]
    fn rust_console_private_mode_reply_length_preserves_key_dispatch_boundary() {
        for reply in [
            b"\x1b[?7;1$y".as_slice(),
            b"\x1b[?2026;0$y",
            b"\x1b[?65535;4$y",
        ] {
            for end in 0..reply.len() {
                assert_eq!(
                    TerminalResponses::new(Duration::ZERO)
                        .reply_length(&reply[..end], Duration::ZERO),
                    None
                );
            }
            let mut joined = reply.to_vec();
            joined.extend_from_slice(b"\x1b[Atext");
            assert_eq!(
                TerminalResponses::new(Duration::ZERO).reply_length(&joined, Duration::ZERO),
                Some(reply.len())
            );
        }
        for bytes in [
            b"\x1b[A".as_slice(),
            b"\x1b[?7;1y",
            b"\x1b[?7;5$y",
            b"\x1b[?65536;1$y",
            b"\x1b[?0000007;1$y",
            b"\x1b[?;1$y",
            b"\x1b[?7;1;2$y",
            b"\x1b[?7;1$x",
            b"\x1b[?7;1$Ay",
        ] {
            assert_eq!(
                TerminalResponses::new(Duration::ZERO).reply_length(bytes, Duration::ZERO),
                None
            );
        }
    }
}
