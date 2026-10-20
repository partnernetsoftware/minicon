//! Reusable terminal state and streaming mechanisms, with an experimental
//! owned Windows console service. The `terminal` module is OS/ISA-neutral;
//! native ConDrv and console API exports currently exist only on Windows.
//!
//! The service is not yet a complete ConPTY/conhost replacement or the default
//! product backend. Native lifecycle and compatibility evidence belong to
//! prd/PRD_02_34_windows_console_backend.md; reusable library ownership belongs
//! to prd/PRD_02_35_terminal_library.md, indexed by the reuse map in
//! prd/PRD_02_28_shared_core.md. Portable session/input composition is planned;
//! it is not implied by the common state and streaming API.

pub mod terminal;

#[cfg(windows)]
mod client;
#[cfg(windows)]
mod control;
#[cfg(windows)]
mod driver;
#[cfg(windows)]
mod host;

#[cfg(windows)]
pub use client::{ClientOptions, ConsoleChild};
#[cfg(windows)]
pub use driver::{DriverRequest, DriverSession, RequestDescriptor};
#[cfg(windows)]
pub use host::{ConsoleHost, ConsoleInput, DispatchReceipt, InputReadState};
