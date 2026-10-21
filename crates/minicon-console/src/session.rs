//! Portable session contract vocabulary: the OS/ISA-neutral scalar types a
//! terminal session exposes to its consumers — errors, sizes, completion and
//! cleanup receipts, input ownership and native-key identities.
//!
//! Moved here from `minicon-platform`'s `contract::pty` (2026-10-21) as the
//! "migrate neutral dependencies first" step of the library sequence
//! (plan/plan-terminal-library.md); the platform facade re-exports these
//! types, so existing `minicon_platform::pty` consumers keep one type
//! identity. These are contract types only: no session implementation, no
//! native handles, `std` as the only dependency. The portable session
//! *interface* remains unpublished until the integration acceptance contract
//! in prd/PRD_02_35_terminal_library.md is demonstrated against both the
//! Windows and POSIX providers.

use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum PtyError {
    Unsupported {
        operation: &'static str,
        reason: String,
    },
    Failed {
        operation: &'static str,
        code: &'static str,
        message: String,
    },
}

impl PtyError {
    pub fn unsupported(operation: &'static str, reason: impl fmt::Display) -> Self {
        Self::Unsupported {
            operation,
            reason: reason.to_string(),
        }
    }

    pub fn failed(operation: &'static str, code: &'static str, error: impl fmt::Display) -> Self {
        Self::Failed {
            operation,
            code,
            message: error.to_string(),
        }
    }
}

impl fmt::Display for PtyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported { operation, reason } => {
                write!(formatter, "PTY {operation} unsupported: {reason}")
            }
            Self::Failed {
                operation,
                code,
                message,
            } => write!(formatter, "PTY {operation} failed ({code}): {message}"),
        }
    }
}

impl std::error::Error for PtyError {}

pub type PtyResult<T> = Result<T, PtyError>;

/// Read-only aggregate for this session's native console, including its
/// attached descendants. Admitted reads are validated waiters; deferred reads
/// are retained calls awaiting output admission. This snapshot contains no
/// process/file identity and grants no authority over future reads.
/// Backends without this observation capability return Unsupported.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct NativeInputReadState {
    pub admitted_text: u32,
    pub admitted_records: u32,
    pub deferred_text: u32,
    pub deferred_records: u32,
}

/// Evidence returned only after the native PTY containment owner has stopped
/// every process it can still identify and has observed that containment empty.
///
/// The counts intentionally omit process identifiers: callers need proof of
/// bounded cleanup, not a new source of mutable PID authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PtyCleanupReceipt {
    pub containment: &'static str,
    pub members_observed: u32,
    pub members_terminated: u32,
    pub verified_empty: bool,
}

/// The bounded foreground-process-group signals whose identity can be derived
/// from a retained PTY master on POSIX hosts.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum PtyForegroundSignal {
    Interrupt,
    Terminate,
    Stop,
    Continue,
}

impl PtyForegroundSignal {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Interrupt => "interrupt",
            Self::Terminate => "terminate",
            Self::Stop => "stop",
            Self::Continue => "continue",
        }
    }
}

/// Evidence from one native foreground-process-group signal operation.
///
/// Process identifiers are deliberately omitted. The retained PTY master is
/// the target authority; the counts and postcondition are bounded evidence,
/// not reusable authority for a later effect.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PtyForegroundSignalReceipt {
    pub containment: &'static str,
    pub signal: &'static str,
    pub members_observed: u32,
    pub members_retained_for_verification: u32,
    pub delivered: bool,
    pub verified: bool,
    pub postcondition: &'static str,
}

/// A platform-neutral terminal key whose native console semantics cannot
/// always be represented faithfully as bytes written to a PTY stream.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum NativeTerminalKey {
    /// The terminal cursor-up key.
    Up,
    /// The terminal cursor-down key.
    Down,
    /// Ctrl+Break. Requires a backend with control-key delivery support;
    /// unavailable transports return Unsupported instead of writing key bytes.
    Break,
}

/// The input protocol currently owned by the program attached to a PTY.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum NativeInputOwnership {
    /// The operating system's line-oriented console editor owns input.
    Cooked,
    /// The program consumes virtual-terminal input sequences.
    RawVt,
    /// The program consumes native console input records.
    RawNative,
}

/// Terminal size in cells. Rows and columns have exactly one meaning here;
/// a pixel size is a different contract and never rides these fields.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TerminalSize {
    pub rows: u16,
    pub cols: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct ProcessId(u32);

impl ProcessId {
    pub fn new(raw: u32) -> Result<Self, InvalidProcessId> {
        if raw == 0 {
            return Err(InvalidProcessId(raw));
        }
        Ok(Self(raw))
    }
    pub const fn as_u32(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidProcessId(u32);

impl std::fmt::Display for InvalidProcessId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "invalid process id: {}", self.0)
    }
}
impl std::error::Error for InvalidProcessId {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_failures_distinguish_unsupported_and_failed() {
        let unsupported = PtyError::unsupported("spawn", "backend unavailable");
        let failed = PtyError::failed("resize", "pty_resize_failed", "invalid dimensions");

        assert!(matches!(unsupported, PtyError::Unsupported { .. }));
        assert!(unsupported.to_string().contains("spawn unsupported"));
        assert!(matches!(failed, PtyError::Failed { .. }));
        assert!(failed.to_string().contains("pty_resize_failed"));
    }

    #[test]
    fn native_terminal_keys_are_platform_neutral_values() {
        assert_ne!(NativeTerminalKey::Up, NativeTerminalKey::Down);
        assert_eq!(NativeTerminalKey::Up, NativeTerminalKey::Up);
    }

    #[test]
    fn foreground_signal_names_are_stable() {
        assert_eq!(PtyForegroundSignal::Interrupt.as_str(), "interrupt");
        assert_eq!(PtyForegroundSignal::Terminate.as_str(), "terminate");
        assert_eq!(PtyForegroundSignal::Stop.as_str(), "stop");
        assert_eq!(PtyForegroundSignal::Continue.as_str(), "continue");
    }

    #[test]
    fn native_input_ownership_has_no_unknown_state() {
        let states = [
            NativeInputOwnership::Cooked,
            NativeInputOwnership::RawVt,
            NativeInputOwnership::RawNative,
        ];

        assert_eq!(states.len(), 3);
        assert_ne!(states[0], states[1]);
        assert_ne!(states[1], states[2]);
    }

    #[test]
    fn process_ids_refuse_zero() {
        assert!(ProcessId::new(0).is_err());
        assert_eq!(ProcessId::new(7).expect("nonzero pid").as_u32(), 7);
    }
}
