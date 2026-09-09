//! Grok Build CLI executable discovery.
//!
//! Precedence mirrors the TypeScript engine (`executable: "grok"` default in
//! `modules/engines/src/grok/engine.ts`) plus the sibling Codex worker
//! override convention (`ARTISAN_CODEX_EXECUTABLE` in
//! `modules/engines/src/codex/executable.ts`):
//!
//! 1. explicit `ARTISAN_GROK_EXECUTABLE` override (verbatim, spaces intact);
//! 2. normal installation/`PATH` resolution.
//!
//! This module performs no process spawns and reads the live environment only
//! through [`resolve_live`]; everything else takes fixture inputs so tests
//! never touch the host.

use std::path::{Path, PathBuf};

/// Native override convention mirroring the sibling Codex/Claude workers.
/// A non-blank value here wins over every lookup below.
pub const GROK_EXECUTABLE_ENV: &str = "ARTISAN_GROK_EXECUTABLE";

/// Default Grok Build binary name from the TypeScript engine definition.
pub const GROK_BINARY_NAME: &str = "grok";

/// Version-report arguments (the shared ACP core defaults to `["--version"]`;
/// Grok keeps that default per `modules/engines/src/acp/engine.ts`).
pub const GROK_VERSION_ARGS: &[&str] = &["--version"];

/// Non-billable auth probe arguments from the TypeScript Grok definition.
pub const GROK_AUTH_PROBE_ARGS: &[&str] = &["--no-auto-update", "models"];

/// Where a resolved Grok binary came from. Override and `PATH` lookup stay
/// distinct so later packets can explain the selection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GrokResolveSource {
    ExplicitOverride,
    PathLookup,
}

/// A Grok binary resolved through [`resolve_grok_binary`] or [`resolve_live`].
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

    /// Returns whether the path came from the explicit override or `PATH`.
    #[must_use]
    pub const fn source(&self) -> GrokResolveSource {
        self.source
    }
}

/// Candidate binary file names for the host platform.
fn candidate_binary_names() -> &'static [&'static str] {
    #[cfg(windows)]
    {
        &["grok.exe", "grok"]
    }
    #[cfg(not(windows))]
    {
        &["grok"]
    }
}

/// Trims an explicit override value. Blank values fall through to `PATH`
/// lookup instead of producing an empty path.
#[must_use]
pub fn explicit_override(raw: Option<&str>) -> Option<PathBuf> {
    raw.map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Searches `PATH`-style directories for the Grok binary. Candidate paths
/// containing spaces flow through untouched as [`PathBuf`] (no splitting or
/// quoting); the first existing candidate wins.
pub fn find_grok_on_path(
    path_var: Option<&str>,
    exists: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let path_var = path_var.filter(|value| !value.trim().is_empty())?;
    for directory in std::env::split_paths(path_var) {
        if directory.as_os_str().is_empty() {
            continue;
        }
        for name in candidate_binary_names() {
            let candidate = directory.join(name);
            if exists(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

/// Resolves the Grok binary from fixture inputs: the explicit override wins;
/// otherwise normal installation/`PATH` resolution applies. Returns `None`
/// when no binary is found (the live probe reports this as not installed,
/// never as authenticated).
#[must_use]
pub fn resolve_grok_binary(
    override_raw: Option<&str>,
    path_var: Option<&str>,
    exists: impl Fn(&Path) -> bool,
) -> Option<ResolvedGrokBinary> {
    if let Some(path) = explicit_override(override_raw) {
        return Some(ResolvedGrokBinary {
            path,
            source: GrokResolveSource::ExplicitOverride,
        });
    }
    find_grok_on_path(path_var, exists).map(|path| ResolvedGrokBinary {
        path,
        source: GrokResolveSource::PathLookup,
    })
}

/// Resolves the Grok binary from the live process environment
/// (`ARTISAN_GROK_EXECUTABLE`, then `PATH` with a filesystem existence
/// check). Returns `None` when no `grok` CLI is installed.
#[must_use]
pub fn resolve_live() -> Option<ResolvedGrokBinary> {
    let override_raw = std::env::var(GROK_EXECUTABLE_ENV).ok();
    let path_var = std::env::var_os("PATH").and_then(|value| value.into_string().ok());
    resolve_grok_binary(override_raw.as_deref(), path_var.as_deref(), |path| {
        path.is_file()
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
