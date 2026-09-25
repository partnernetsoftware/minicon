//! The opencode-go-compatible harness backend.
//!
//! H1 forbids assuming the DeepSeek adapter covers this backend, so this
//! module owns its own request body, its own reply parsing and its own
//! bounds. It reuses only the `Transport` seam from `harness_wire`, never
//! that module's DeepSeek codec.
