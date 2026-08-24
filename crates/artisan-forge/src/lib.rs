//! Library surface of the standalone Forge host.
//!
//! The binary in `main.rs` is thin; every behavior lives here so the host is
//! testable in-process against real sockets.

pub mod config;
pub mod host;
pub mod lease;
pub mod state;
