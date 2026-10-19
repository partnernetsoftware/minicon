//! Experimental owned Windows console service.
//!
//! This crate is not yet a replacement for ConPTY/conhost. The initial slice
//! establishes a real, owned ConDrv server and a limited console API service.
//! Actual cmd I/O, Win32 input records and job-owned cleanup are tested.
//! Current-object lifetime, scoped processed Ctrl+C and experimental PTY
//! integration are implemented. Full signals, screen semantics and product
//! selection remain incomplete; this is not a default backend.
//! Architecture, provenance and acceptance: prd/PRD_02_34_windows_console_backend.md.

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
pub use host::{ConsoleHost, ConsoleInput, DispatchReceipt};
