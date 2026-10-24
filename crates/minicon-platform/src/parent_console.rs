//! OS-neutral parent-console output service.
//!
//! This deliberately does not imply process inventory, control, observation,
//! spawning, or containment. GUI-subsystem launchers can report diagnostics
//! without enabling the aggregate `process` capability.

use crate::selected::parent_console as adapter;

/// Keeps an existing parent console attached for a foreground CLI lifetime.
/// Does not allocate a console or suppress Ctrl-C. On Unix this is a no-op.
#[must_use = "keep the session alive while observing foreground interrupts"]
pub struct ParentConsoleSession {
    _inner: adapter::Session,
}

impl ParentConsoleSession {
    /// `None` means there is no attachable parent console (or it is already
    /// attached). Inherited output pipes remain usable without attachment.
    pub fn attach() -> Option<Self> {
        adapter::attach_session().map(|inner| Self { _inner: inner })
    }
}

/// Write one diagnostic line to stderr or an already-existing parent console.
///
/// This never allocates a new console and reports best-effort success.
pub fn write_stderr(message: &str) -> bool {
    adapter::write_stderr(message)
}

/// Write one CLI line to stdout or an already-existing parent console.
pub fn write_stdout(message: &str) -> bool {
    adapter::write_stdout(message)
}
