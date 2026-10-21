//! PTY-neutral scalar types shared by native session adapters.
//!
//! The vocabulary itself is owned by `minicon_console::session` since
//! 2026-10-21 (the "migrate neutral dependencies first" step of the library
//! sequence, plan/plan-terminal-library.md). This module re-exports it so
//! every existing `minicon_platform::pty` consumer keeps the same type
//! identity; the facade and adapters keep importing `crate::contract::pty`.

pub use minicon_console::session::{
    InvalidProcessId, NativeInputOwnership, NativeInputReadState, NativeTerminalKey, ProcessId,
    PtyCleanupReceipt, PtyError, PtyForegroundSignal, PtyForegroundSignalReceipt, PtyResult,
    TerminalSize,
};
