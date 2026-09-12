#![forbid(unsafe_code)]

pub mod account_usage;
pub mod account_usage_claude;
pub mod account_usage_codex;
pub mod account_usage_resolve;

pub use account_usage::{
    CallError, CliLaunch, CliResolveInput, ExchangeBounds, JsonRpcSession, ProviderError,
    ProviderUsage, USAGE_MAX_LINE_BYTES, USAGE_MAX_QUEUED_LINES, USAGE_MAX_SKIPPED_FRAMES,
    USAGE_MAX_TOTAL_BYTES, USAGE_TEARDOWN_GRACE, USAGE_TEARDOWN_POLL, UsageReaderError,
    resolve_claude_cli, resolve_cli_with, resolve_codex_cli,
};
pub use account_usage_claude::{
    CLAUDE_USAGE_ARGS, CLAUDE_USAGE_MAX_BYTES, CLAUDE_USAGE_TIMEOUT, ClaudeUsageConfig,
    parse_claude_usage_windows, read_claude_usage,
};
pub use account_usage_codex::{
    CODEX_USAGE_OVERALL_TIMEOUT, CodexAccount, CodexUsageConfig, map_codex_account,
    map_codex_rate_limits, read_codex_usage,
};

// `claude.rs` / `codex.rs` (the certified launch authorities, wired as
// `*_authority` below) coexist with the `claude/` / `codex/` discovery
// directories, so the directory modules need explicit paths: a plain
// `pub mod claude;` / `pub mod codex;` would resolve to both `*.rs` and
// `*/mod.rs` (E0583).
#[path = "claude/mod.rs"]
pub mod claude;
#[path = "claude.rs"]
mod claude_authority;
#[path = "codex/mod.rs"]
pub mod codex;
#[path = "codex.rs"]
mod codex_authority;
pub mod cursor;
#[path = "install.rs"]
mod engine_core;
pub mod grok;
#[path = "files.rs"]
mod io;
#[path = "profile.rs"]
mod resolver;
#[cfg(windows)]
mod windows_private;

pub use claude_authority::{
    CLAUDE_EXECUTABLE_ENV_VAR, CLAUDE_MINIMUM_CLI_VERSION, CLAUDE_NATIVE_CONTINUATION_VERSION,
    CLAUDE_PROTOCOL_VERSION, CLAUDE_TRANSPORT, NativeClaudeAuthority, NativeClaudeLaunchError,
    VerifiedClaudeLaunch, compare_claude_versions,
};
pub use codex_authority::{
    CODEX_APP_SERVER_ARGS, CODEX_MINIMUM_CLI_VERSION, CODEX_OPT_OUT_NOTIFICATION_METHODS,
    CODEX_PROTOCOL_VERSION, CODEX_TRANSPORT, NativeCodexAuthority, NativeCodexLaunchError,
    VerifiedCodexLaunch, compare_codex_versions,
};
pub use engine_core::{
    NativeOpenCode2Authority, NativeOpenCode2Error, NativeOpenCode2InstallLock,
    NativeOpenCode2InstallLockError, NativeOpenCode2InstallPathError, NativeOpenCode2InstallPaths,
    NativeOpenCode2InstallSpec, NativeOpenCode2State, NativeOpenCode2StateError,
    OpenCode2Inspection, ResolvedOpenCode2Generation, platform_supported,
};
pub use io::{
    AtomicReplaceOutcome, NativeFileError, VerifiedFileIdentity, ensure_directory,
    ensure_private_directory, read_bounded, replace_file, validate_private_directory,
    verify_directory, verify_file,
};
pub use resolver::{
    NativeOpenCode2ProfileError, NativeOpenCode2ProfileLaunchError, OpenCode2Profile,
    ProfileHomeKind, ProfileRegistrationOutcome, VerifiedOpenCode2ProfileLaunch,
};
