//! Experimental owned Windows console service.
//!
//! This crate is not yet a replacement for ConPTY/conhost. The initial slice
//! establishes a real, owned ConDrv server, its input event and a driver
//! reference. Client attachment and API dispatch remain unimplemented.
//! Architecture, provenance and acceptance: prd/PRD_02_34_windows_console_backend.md.

#[cfg(windows)]
mod driver;

#[cfg(windows)]
pub use driver::{DriverRequest, DriverSession, RequestDescriptor};
