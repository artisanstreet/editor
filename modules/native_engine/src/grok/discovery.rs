//! Grok Build CLI executable resolution and version parsing.
//!
//! The executable is the Forge-managed Grok generation (see
//! `crate::engine_core`) or the absolute `ARTISAN_GROK_EXECUTABLE` developer
//! override; `PATH` is never searched. xAI publishes no digest for its
//! binaries, so no managed Grok generation exists today and only the
//! override can resolve.

use std::path::{Path, PathBuf};

use crate::engine_core::{LaunchSource, ManagedEngine, resolve_launch_target};

/// The developer override: an absolute executable path, reported as an
/// override in engine status.
pub const GROK_EXECUTABLE_ENV: &str = ManagedEngine::Grok.override_env();

/// Default Grok Build binary name from the TypeScript engine definition.
pub const GROK_BINARY_NAME: &str = "grok";

/// Version-report arguments (the shared ACP core defaults to `["--version"]`;
/// Grok keeps that default per `modules/engines/src/acp/engine.ts`).
pub const GROK_VERSION_ARGS: &[&str] = &["--version"];

/// Non-billable auth probe arguments from the TypeScript Grok definition.
pub const GROK_AUTH_PROBE_ARGS: &[&str] = &["--no-auto-update", "models"];

/// Where a resolved Grok binary came from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GrokResolveSource {
    ExplicitOverride,
    Managed,
}

/// A Grok binary resolved through [`resolve_live`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedGrokBinary {
    path: PathBuf,
    source: GrokResolveSource,
}

impl ResolvedGrokBinary {
    /// Returns the resolved executable path (verbatim; never split or quoted).
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns whether the path is the managed generation or the override.
    #[must_use]
    pub const fn source(&self) -> GrokResolveSource {
        self.source
    }
}

/// Resolves the Grok binary for the registered Forge. Returns `None` when
/// Grok is not installed (the live probe reports this as not installed,
/// never as authenticated).
#[must_use]
pub fn resolve_live() -> Option<ResolvedGrokBinary> {
    let target = resolve_launch_target(ManagedEngine::Grok).ok()?;
    Some(ResolvedGrokBinary {
        path: target.executable().to_path_buf(),
        source: match target.source() {
            LaunchSource::Managed => GrokResolveSource::Managed,
            LaunchSource::Override => GrokResolveSource::ExplicitOverride,
        },
    })
}

fn is_word_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Parses the installed version with the TypeScript version regex
/// (`/\bgrok\s+(\d+\.\d+\.\d+(?:-[0-9A-Za-z._-]+)?)/i`): a case-insensitive
/// `grok` word followed by whitespace and `MAJOR.MINOR.PATCH` with an
/// optional `-prerelease` suffix. Returns the version substring only.
#[must_use]
pub fn parse_grok_version(output: &str) -> Option<String> {
    let bytes = output.as_bytes();
    let mut index = 0_usize;
    while index + 4 <= bytes.len() {
        if bytes[index..index + 4].eq_ignore_ascii_case(b"grok")
            && (index == 0 || !is_word_char(bytes[index - 1]))
            && index + 4 < bytes.len()
            && bytes[index + 4].is_ascii_whitespace()
        {
            let mut cursor = index + 4;
            while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
                cursor += 1;
            }
            if let Some(version) = parse_semver_at(bytes, cursor) {
                return Some(version);
            }
        }
        index += 1;
    }
    None
}

/// Parses `MAJOR.MINOR.PATCH` with an optional `-prerelease` suffix at `start`.
/// A trailing `-` with no suffix accepts the base version, matching the
/// optional regex group. All consumed bytes are ASCII, so slicing is safe.
fn parse_semver_at(bytes: &[u8], start: usize) -> Option<String> {
    let mut cursor = start;
    for part in 0..3 {
        let digits_start = cursor;
        while cursor < bytes.len() && bytes[cursor].is_ascii_digit() {
            cursor += 1;
        }
        if cursor == digits_start {
            return None;
        }
        if part < 2 {
            if cursor >= bytes.len() || bytes[cursor] != b'.' {
                return None;
            }
            cursor += 1;
        }
    }
    if cursor < bytes.len() && bytes[cursor] == b'-' {
        let mut suffix_end = cursor + 1;
        while suffix_end < bytes.len()
            && (bytes[suffix_end].is_ascii_alphanumeric()
                || matches!(bytes[suffix_end], b'.' | b'_' | b'-'))
        {
            suffix_end += 1;
        }
        if suffix_end > cursor + 1 {
            cursor = suffix_end;
        }
    }
    core::str::from_utf8(&bytes[start..cursor])
        .ok()
        .map(str::to_owned)
}
