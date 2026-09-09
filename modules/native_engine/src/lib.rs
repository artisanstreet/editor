#![forbid(unsafe_code)]

pub mod account_usage;
pub mod account_usage_claude;
pub mod account_usage_codex;

pub use account_usage::{
    CallError, ExchangeBounds, JsonRpcSession, ProviderError, ProviderUsage, USAGE_MAX_LINE_BYTES,
    USAGE_MAX_SKIPPED_FRAMES, USAGE_MAX_TOTAL_BYTES, UsageReaderError,
};
pub use account_usage_claude::{
    CLAUDE_USAGE_ARGS, CLAUDE_USAGE_MAX_BYTES, CLAUDE_USAGE_TIMEOUT, ClaudeUsageConfig,
    parse_claude_usage_windows, read_claude_usage,
};
pub use account_usage_codex::{
    CODEX_USAGE_OVERALL_TIMEOUT, CodexAccount, CodexUsageConfig, map_codex_account,
    map_codex_rate_limits, read_codex_usage,
};

#[path = "install.rs"]
mod engine_core;
#[path = "files.rs"]
mod io;
#[path = "profile.rs"]
mod resolver;
#[cfg(windows)]
mod windows_private;

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
