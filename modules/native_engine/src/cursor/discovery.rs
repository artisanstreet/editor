//! Cursor (`cursor-agent`) executable resolution and version parsing.
//!
//! The executable is the Forge-managed Cursor generation (see
//! `crate::engine_core`) or the absolute `ARTISAN_CURSOR_EXECUTABLE`
//! developer override; `PATH` is never searched. Cursor publishes no digest
//! for its agent package, so no managed Cursor generation exists today and
//! only the override can resolve.

use std::path::{Path, PathBuf};

use crate::engine_core::{LaunchSource, ManagedEngine, resolve_launch_target};

/// The developer override: an absolute executable path, reported as an
/// override in engine status.
pub const CURSOR_EXECUTABLE_ENV: &str = ManagedEngine::Cursor.override_env();

/// Installed binary name named by the worker contract.
pub const CURSOR_BINARY_NAME: &str = "cursor-agent";

/// Version-report arguments (the shared ACP core defaults to `["--version"]`;
/// Cursor keeps that default per `modules/engines/src/acp/engine.ts`).
pub const CURSOR_VERSION_ARGS: &[&str] = &["--version"];

/// Non-billable auth probe arguments from the TypeScript Cursor definition
/// (`auth_probe_args: ["status"]` in `modules/engines/src/cursor/engine.ts`).
pub const CURSOR_AUTH_PROBE_ARGS: &[&str] = &["status"];

/// Where a resolved Cursor binary came from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CursorResolveSource {
    ExplicitOverride,
    Managed,
}

/// A Cursor binary resolved through [`resolve_live`].
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

    /// Returns whether the path is the managed generation or the override.
    #[must_use]
    pub const fn source(&self) -> CursorResolveSource {
        self.source
    }
}

/// Resolves the Cursor binary for the registered Forge. Returns `None` when
/// Cursor is not installed.
#[must_use]
pub fn resolve_live() -> Option<ResolvedCursorBinary> {
    let target = resolve_launch_target(ManagedEngine::Cursor).ok()?;
    Some(ResolvedCursorBinary {
        path: target.executable().to_path_buf(),
        source: match target.source() {
            LaunchSource::Managed => CursorResolveSource::Managed,
            LaunchSource::Override => CursorResolveSource::ExplicitOverride,
        },
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
    while bytes.get(cursor).is_some_and(|byte| is_suffix_char(*byte)) {
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
        if let Some((suffix_start, greedy_end)) = match_version_at(bytes, index)
            && (index == 0 || !is_word_char(bytes[index - 1]))
        {
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
        index += 1;
    }
    None
}
