//! Experimental owned Windows console service.
//!
//! This crate is not yet a replacement for ConPTY/conhost. The initial slice
//! establishes a real, owned ConDrv server and a limited console API service.
//! Actual cmd output and exit are tested; general input, native object creation,
//! complete screen semantics and product PTY integration remain unimplemented.
//! Architecture, provenance and acceptance: prd/PRD_02_34_windows_console_backend.md.

#[cfg(windows)]
mod driver;
#[cfg(windows)]
mod host;

#[cfg(windows)]
pub use driver::{DriverRequest, DriverSession, RequestDescriptor};
#[cfg(windows)]
pub use host::{ConsoleHost, DispatchReceipt};
