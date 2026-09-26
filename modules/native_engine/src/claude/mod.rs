//! Finite Claude Code native executable discovery and readiness probe.
//!
//! This module ports the TypeScript Claude readiness behavior behind the same
//! external executable boundary used for actual runs:
//! `modules/engines/src/claude/cli-engine.ts`,
//! `modules/engines/src/claude/probe.ts`, and
//! `modules/engines/src/claude/descriptor.ts`.
//!
//! Scope is deliberately narrow: take the `claude` executable from an
//! explicit command or the Forge-managed generation (never `PATH`), then run only the bounded, non-billable `--version` and
//! `auth status` probes. No prompt, inference, session, account, usage, or
//! credential mutation lives here.
//!
//! There is intentionally no shared engine trait or registry integration in
//! this packet. The typed [`probe::ClaudeProbeResult`] is the per-engine
//! result pending controller-owned registry work.

pub mod discovery;
pub mod probe;

pub use discovery::{
    CLAUDE_EXECUTABLE_ENV_VAR, ClaudeDiscoveryError, ClaudeExecutable, ClaudeExecutableSource,
    MAX_EXECUTABLE_ARG_BYTES, MAX_EXECUTABLE_ARGS, MAX_EXECUTABLE_VALUE_BYTES,
    discover_claude_executable, select_override_value,
};
pub use probe::{
    CLAUDE_ENGINE_ID, CLAUDE_PROTOCOL_VERSION, CLAUDE_TRANSPORT, ClaudeAuthState,
    ClaudeAuthentication, ClaudeProbeError, ClaudeProbeOptions, ClaudeProbePhase,
    ClaudeProbeResult, ClaudeProbeRunner, DEFAULT_AUTH_REASON_UNAVAILABLE, DEFAULT_AUTH_TIMEOUT,
    DEFAULT_MAX_STDERR_BYTES, DEFAULT_MAX_STDOUT_BYTES, DEFAULT_VERSION_TIMEOUT,
    MAX_AUTH_REASON_BYTES, MAX_PROBE_TIMEOUT, NATIVE_CONTINUATION_VERSION, classify_authentication,
    parse_auth_logged_in, parse_claude_version, sanitize_auth_reason,
};
