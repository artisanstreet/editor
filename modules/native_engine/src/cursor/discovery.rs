//! Cursor (`cursor-agent` / `agent`) executable discovery.
//!
//! Precedence mirrors the TypeScript engine (`executable: "agent.cmd"` on
//! win32, `"agent"` elsewhere in `modules/engines/src/cursor/engine.ts`)
//! plus the sibling worker override convention (`ARTISAN_CODEX_EXECUTABLE`
//! in `modules/engines/src/codex/executable.ts`, also used by the Claude and
//! Grok workers):
//!
//! 1. explicit `ARTISAN_CURSOR_EXECUTABLE` override (verbatim, spaces intact);
//! 2. normal installation/`PATH` resolution, preferring `cursor-agent` and
//!    falling back to the TypeScript default `agent` names.
//!
//! This module performs no process spawns and reads the live environment only
//! through [`resolve_live`]; everything else takes fixture inputs so tests
//! never touch the host.

use std::path::{Path, PathBuf};

/// Native override convention mirroring the sibling Codex/Claude/Grok
/// workers. A non-blank value here wins over every lookup below.
pub const CURSOR_EXECUTABLE_ENV: &str = "ARTISAN_CURSOR_EXECUTABLE";

/// Installed binary name named by the worker contract.
pub const CURSOR_BINARY_NAME: &str = "cursor-agent";

/// Version-report arguments (the shared ACP core defaults to `["--version"]`;
/// Cursor keeps that default per `modules/engines/src/acp/engine.ts`).
pub const CURSOR_VERSION_ARGS: &[&str] = &["--version"];

/// Non-billable auth probe arguments from the TypeScript Cursor definition
/// (`auth_probe_args: ["status"]` in `modules/engines/src/cursor/engine.ts`).
pub const CURSOR_AUTH_PROBE_ARGS: &[&str] = &["status"];

/// Where a resolved Cursor binary came from. Override and `PATH` lookup stay
/// distinct so later packets can explain the selection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CursorResolveSource {
    ExplicitOverride,
    PathLookup,
}

/// A Cursor binary resolved through [`resolve_cursor_binary`] or
/// [`resolve_live`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedCursorBinary {
    path: PathBuf,
    source: CursorResolveSource,
}

impl ResolvedCursorBinary {
    /// Returns the resolved executable path (verbatim; never split or quoted).
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns whether the path came from the explicit override or `PATH`.
    #[must_use]
    pub const fn source(&self) -> CursorResolveSource {
        self.source
    }
}

/// Candidate binary file names for the host platform, in lookup order.
///
/// `cursor-agent` leads per the worker contract; the trailing `agent` names
/// are the TypeScript default executable (`agent.cmd` on Windows, `agent`
/// elsewhere in `modules/engines/src/cursor/engine.ts`).
fn candidate_binary_names() -> &'static [&'static str] {
    #[cfg(windows)]
    {
        &[
            "cursor-agent.exe",
            "cursor-agent",
            "agent.cmd",
            "agent.exe",
            "agent",
        ]
    }
    #[cfg(not(windows))]
    {
        &["cursor-agent", "agent"]
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

/// Searches `PATH`-style directories for the Cursor binary. Candidate paths
/// containing spaces flow through untouched as [`PathBuf`] (no splitting or
/// quoting); the first existing candidate wins.
#[must_use]
pub fn find_cursor_on_path(
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

/// Resolves the Cursor binary from fixture inputs: the explicit override
/// wins; otherwise normal installation/`PATH` resolution applies. Returns
/// `None` when no binary is found (the live probe reports this as not
/// installed, never as authenticated).
#[must_use]
pub fn resolve_cursor_binary(
    override_raw: Option<&str>,
    path_var: Option<&str>,
    exists: impl Fn(&Path) -> bool,
) -> Option<ResolvedCursorBinary> {
    if let Some(path) = explicit_override(override_raw) {
        return Some(ResolvedCursorBinary {
            path,
            source: CursorResolveSource::ExplicitOverride,
        });
    }
    find_cursor_on_path(path_var, exists).map(|path| ResolvedCursorBinary {
        path,
        source: CursorResolveSource::PathLookup,
    })
}

/// Resolves the Cursor binary from the live process environment
/// (`ARTISAN_CURSOR_EXECUTABLE`, then `PATH` with a filesystem existence
/// check). Returns `None` when no Cursor CLI is installed.
#[must_use]
pub fn resolve_live() -> Option<ResolvedCursorBinary> {
    let override_raw = std::env::var(CURSOR_EXECUTABLE_ENV).ok();
    let path_var = std::env::var_os("PATH").and_then(|value| value.into_string().ok());
    resolve_cursor_binary(override_raw.as_deref(), path_var.as_deref(), |path| {
        path.is_file()
    })
}

const fn is_word_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

const fn is_suffix_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'_' || byte == b'-'
}

/// Matches `\d{4}\.\d{1,2}\.\d{1,2}-[0-9A-Za-z._-]+` at `start`, returning the
/// suffix start and the end offset of the greedy match. Digit groups that run
/// longer than the pattern allows fail here (the separator check rejects the
/// extra digit), exactly like the TypeScript regex backtracking to no match
/// at that position.
fn match_version_at(bytes: &[u8], start: usize) -> Option<(usize, usize)> {
    let mut cursor = start;
    for _ in 0..4 {
        if !bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
            return None;
        }
        cursor += 1;
    }
    for _ in 0..2 {
        if bytes.get(cursor) != Some(&b'.') {
            return None;
        }
        cursor += 1;
        let digits_start = cursor;
        while bytes.get(cursor).is_some_and(u8::is_ascii_digit) && cursor - digits_start < 2 {
            cursor += 1;
        }
        if cursor == digits_start {
            return None;
        }
        if bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
            return None;
        }
    }
    if bytes.get(cursor) != Some(&b'-') {
        return None;
    }
    cursor += 1;
    let suffix_start = cursor;
    while bytes.get(cursor).is_some_and(is_suffix_char) {
        cursor += 1;
    }
    if cursor == suffix_start {
        return None;
    }
    Some((suffix_start, cursor))
}

/// Parses the installed version with the TypeScript version regex
/// (`/\b(\d{4}\.\d{1,2}\.\d{1,2}-[0-9A-Za-z._-]+)\b/` in
/// `modules/engines/src/cursor/engine.ts`): a word boundary, a four-digit
/// year, two one-to-two-digit groups, and a mandatory `-suffix`. Returns the
/// version substring only. The trailing word boundary emulates regex
/// backtracking by shrinking a trailing run of non-word suffix characters.
#[must_use]
pub fn parse_cursor_version(output: &str) -> Option<String> {
    let bytes = output.as_bytes();
    let mut index = 0_usize;
    while index < bytes.len() {
        if let Some((suffix_start, greedy_end)) = match_version_at(bytes, index) {
            if index == 0 || !is_word_char(bytes[index - 1]) {
                let mut end = greedy_end;
                loop {
                    let boundary_ok = if end >= bytes.len() {
                        true
                    } else if is_word_char(bytes[end - 1]) {
                        !is_word_char(bytes[end])
                    } else {
                        is_word_char(bytes[end])
                    };
                    if boundary_ok {
                        return core::str::from_utf8(&bytes[index..end])
                            .ok()
                            .map(str::to_owned);
                    }
                    // Only a trailing run of non-word suffix characters
                    // (`.`/`-`) can gain a boundary by shrinking; anything
                    // else means no match starts here.
                    if end > suffix_start && !is_word_char(bytes[end - 1]) {
                        end -= 1;
                    } else {
                        break;
                    }
                }
            }
        }
        index += 1;
    }
    None
}
