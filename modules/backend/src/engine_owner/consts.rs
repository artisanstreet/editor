//! Shared exact constants for backend engine and directory boundaries.
//!
//! Each value is defined exactly once here and imported by name at every use
//! site. The values are load-bearing (Windows creation flags, fixture
//! watchdog endings, bounded provider identities, native engine tags), so
//! call sites must never fork or re-type them.

#![forbid(unsafe_code)]

/// Windows process-creation flag that keeps engine children console-free.
#[cfg(windows)]
pub(crate) const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Fixture watchdog exit code: an observed reap with this code is always a
/// containment failure, never a normal ending.
#[cfg(test)]
pub(crate) const WATCHDOG_FAILURE_EXIT: i32 = 99;

/// Structural ceiling for one provider request identity held as a table key.
pub(crate) const MAX_PROVIDER_ID_BYTES: usize = 256;

/// Uppercase hexadecimal digit table for percent-encoded (`%XX`) URIs.
pub(crate) const HEX_UPPER: &[u8; 16] = b"0123456789ABCDEF";

/// Exact native engine identity carried by catalog rows and provider
/// bindings.
pub(crate) const OPENCODE2_ENGINE_ID: &str = "opencode2";
pub(crate) const CODEX_ENGINE_ID: &str = "codex";
pub(crate) const CLAUDE_ENGINE_ID: &str = "claude";
pub(crate) const GROK_ENGINE_ID: &str = "grok";
pub(crate) const CURSOR_ENGINE_ID: &str = "cursor";
pub(crate) const HERMES_ENGINE_ID: &str = "hermes";
