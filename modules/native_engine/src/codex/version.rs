//! Finite native Codex version contract.
//!
//! Pure parsing and comparison helpers matching
//! `modules/engines/src/codex/protocol.ts` (`CodexTransportMetadata`) and
//! the version handling in `modules/engines/src/codex/probe.ts`. No process
//! work happens here; the probe runner supplies bounded `--version` bytes
//! and maps these helpers to typed readiness errors.

use std::cmp::Ordering;

/// Minimum Codex CLI version accepted by the transport.
pub const CODEX_MINIMUM_CLI_VERSION: &str = "0.142.5";

/// CLI version the native continuation path is verified against.
///
/// Recorded as data for the later runtime packet; readiness itself only
/// gates on [`CODEX_MINIMUM_CLI_VERSION`].
pub const CODEX_CONTINUATION_CLI_VERSION: &str = "0.145.0";

/// JSON-RPC initialize method used by the later app-server session packet.
pub const CODEX_INITIALIZE_METHOD: &str = "initialize";

/// Native protocol version carried by the later app-server session packet.
pub const CODEX_PROTOCOL_VERSION: &str = "v1";

/// Transport label matching the TypeScript descriptor.
pub const CODEX_TRANSPORT: &str = "stdio-jsonl";

/// Parses the first `major.minor.patch` version in `--version` output.
///
/// Mirrors `ParseCodexVersion` (`/\b(\d+\.\d+\.\d+)\b/`): the surrounding
/// bytes may contain product names or suffixes; only the first semantic
/// version with word boundaries on both sides counts.
#[must_use]
pub fn parse_codex_version(output: &[u8]) -> Option<String> {
    find_dotted_version(output, false)
}

/// Parses the first full version (with optional pre-release/build metadata)
/// in `--version` output.
///
/// Mirrors `ParseCodexContinuationVersion`
/// (`/\b(\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?)\b/`).
#[must_use]
pub fn parse_codex_continuation_version(output: &[u8]) -> Option<String> {
    find_dotted_version(output, true)
}

/// Compares two `major.minor.patch` versions numerically.
///
/// Non-numeric components count as zero, matching the TypeScript behavior of
/// mapping components through `Number` with a `?? 0` fallback for missing
/// parts.
#[must_use]
pub fn compare_semantic_versions(left: &str, right: &str) -> Ordering {
    for index in 0..3 {
        let left_part = numeric_component(left, index);
        let right_part = numeric_component(right, index);
        match left_part.cmp(&right_part) {
            Ordering::Equal => continue,
            order => return order,
        }
    }
    Ordering::Equal
}

/// Reports whether a parsed version meets the minimum CLI version.
#[must_use]
pub fn meets_minimum_version(version: &str) -> bool {
    compare_semantic_versions(version, CODEX_MINIMUM_CLI_VERSION) != Ordering::Less
}

/// Reports whether a parsed version is the verified continuation version.
#[must_use]
pub fn is_continuation_verified_version(version: &str) -> bool {
    version == CODEX_CONTINUATION_CLI_VERSION
}

fn numeric_component(version: &str, index: usize) -> u64 {
    version
        .split('.')
        .nth(index)
        .and_then(|part| {
            let digits: String = part.bytes().take_while(u8::is_ascii_digit).collect();
            if digits.is_empty() {
                None
            } else {
                digits.parse::<u64>().ok()
            }
        })
        .unwrap_or(0)
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn find_dotted_version(output: &[u8], extended: bool) -> Option<String> {
    let mut index = 0;
    while index < output.len() {
        if output[index].is_ascii_digit() && (index == 0 || !is_word_byte(output[index - 1])) {
            if let Some((version, end)) = match_version_at(output, index, extended) {
                if end >= output.len() || !is_word_byte(output[end]) {
                    return Some(version);
                }
            }
        }
        index += 1;
    }
    None
}

fn match_version_at(output: &[u8], start: usize, extended: bool) -> Option<(String, usize)> {
    let mut cursor = start;
    for part in 0..3 {
        let digits_start = cursor;
        while cursor < output.len() && output[cursor].is_ascii_digit() {
            cursor += 1;
        }
        if cursor == digits_start {
            return None;
        }
        if part < 2 {
            if cursor >= output.len() || output[cursor] != b'.' {
                return None;
            }
            cursor += 1;
        }
    }
    if extended {
        cursor = match_prerelease(output, cursor, b'-');
        cursor = match_prerelease(output, cursor, b'+');
    }
    String::from_utf8(output[start..cursor].to_vec())
        .ok()
        .map(|version| (version, cursor))
}

fn match_prerelease(output: &[u8], cursor: usize, marker: u8) -> usize {
    if cursor >= output.len() || output[cursor] != marker {
        return cursor;
    }
    let mut end = cursor + 1;
    let mut length = 0;
    while end < output.len()
        && (output[end].is_ascii_alphanumeric() || output[end] == b'.' || output[end] == b'-')
    {
        end += 1;
        length += 1;
    }
    if length == 0 { cursor } else { end }
}
