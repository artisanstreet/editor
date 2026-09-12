#![allow(clippy::module_name_repetitions)]
#![allow(dead_code)]

use std::ffi::OsString;

use super::ImageMode;

// ---------------------------------------------------------------------------
// Per-engine rows: plain data for grok and cursor only
// ---------------------------------------------------------------------------

/// Explicit launch params mirroring `GrokAcpArgs`/`CursorAcpArgs` inputs.
/// `GrokSettings`/`CursorSettings` do not exist yet, so rows share this
/// struct and each builder interprets its engine-scoped literals.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LaunchArgs {
    pub(crate) model: Option<String>,
    pub(crate) reasoning_effort: Option<String>,
    pub(crate) speed_fast: bool,
    pub(crate) permission: Option<String>,
    pub(crate) write_access: bool,
}

/// Builds grok stdio args, mirroring `GrokAcpArgs`: `--no-auto-update`,
/// optional `--model`/`--reasoning-effort`, plan mode when writes are
/// denied, `auto`/`always-approve` permission mapping, then
/// `agent stdio`.
#[must_use = "arg rows must be passed to the spawn call"]
pub(crate) fn grok_build_args(args: &LaunchArgs) -> Vec<OsString> {
    let mut out = vec![OsString::from("--no-auto-update")];
    if let Some(model) = args.model.as_ref().filter(|model| !model.is_empty()) {
        out.push(OsString::from("--model"));
        out.push(OsString::from(model));
    }
    if let Some(effort) = args
        .reasoning_effort
        .as_ref()
        .filter(|effort| !effort.is_empty())
    {
        out.push(OsString::from("--reasoning-effort"));
        out.push(OsString::from(effort));
    }
    if !args.write_access {
        out.push(OsString::from("--permission-mode"));
        out.push(OsString::from("plan"));
    } else if args.permission.as_deref() == Some("auto") {
        out.push(OsString::from("--permission-mode"));
        out.push(OsString::from("auto"));
    } else if args.permission.as_deref() == Some("always-approve") {
        out.push(OsString::from("--always-approve"));
    }
    out.push(OsString::from("agent"));
    out.push(OsString::from("stdio"));
    out
}

fn has_cursor_effort_suffix(model: &str) -> bool {
    let base = model.strip_suffix("-fast").unwrap_or(model);
    base.rfind('-').is_some_and(|dash| {
        matches!(
            &base[dash + 1..],
            "low" | "medium" | "high" | "xhigh" | "max" | "ultra"
        )
    })
}

fn resolve_cursor_model(args: &LaunchArgs) -> Option<String> {
    let model = args.model.as_ref().filter(|model| !model.is_empty())?;
    if model.contains('[') {
        return Some(model.clone());
    }
    let mut resolved = model.clone();
    if let Some(effort) = args
        .reasoning_effort
        .as_ref()
        .filter(|effort| !effort.is_empty())
        && !has_cursor_effort_suffix(&resolved)
    {
        resolved.push('-');
        resolved.push_str(effort);
    }
    if args.speed_fast && !resolved.ends_with("-fast") {
        resolved.push_str("-fast");
    }
    Some(resolved)
}

/// Builds cursor stdio args, mirroring `CursorAcpArgs` with
/// `ResolveCursorModel`: optional resolved `--model`, ask mode when writes
/// are denied, `--force` mapping, then `acp`.
#[must_use = "arg rows must be passed to the spawn call"]
pub(crate) fn cursor_build_args(args: &LaunchArgs) -> Vec<OsString> {
    let mut out = Vec::new();
    if let Some(model) = resolve_cursor_model(args) {
        out.push(OsString::from("--model"));
        out.push(OsString::from(model));
    }
    if !args.write_access {
        out.push(OsString::from("--mode"));
        out.push(OsString::from("ask"));
    } else if args.permission.as_deref() == Some("force") {
        out.push(OsString::from("--force"));
    }
    out.push(OsString::from("acp"));
    out
}

fn contains_insensitive(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let needle_bytes = needle.as_bytes();
    haystack
        .as_bytes()
        .windows(needle_bytes.len())
        .any(|window| {
            window
                .iter()
                .zip(needle_bytes.iter())
                .all(|(a, b)| a.eq_ignore_ascii_case(b))
        })
}

/// Shared authenticated-output classifier, mirroring the identical grok and
/// cursor `Authenticated` evidence: any `not authenticated`/`not logged in`
/// marker (case-insensitive) means unauthenticated.
#[must_use = "classifier results must gate admission"]
pub(crate) fn default_is_authenticated_output(output: &str) -> bool {
    !contains_insensitive(output, "not authenticated")
        && !contains_insensitive(output, "not logged in")
}

fn parse_semver(bytes: &[u8]) -> Option<String> {
    let mut cursor = 0_usize;
    for part in 0..3 {
        let digits = bytes[cursor..]
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        if digits == 0 {
            return None;
        }
        cursor += digits;
        if part < 2 {
            if bytes.get(cursor) != Some(&b'.') {
                return None;
            }
            cursor += 1;
        }
    }
    if bytes.get(cursor) == Some(&b'-') {
        let suffix = bytes[cursor + 1..]
            .iter()
            .take_while(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
            .count();
        if suffix == 0 {
            return None;
        }
        cursor += 1 + suffix;
    }
    std::str::from_utf8(&bytes[..cursor])
        .ok()
        .map(str::to_owned)
}

/// Parses grok versions, mirroring `/\bgrok\s+(\d+\.\d+\.\d+(?:-…)?)/i`:
/// word-boundary `grok`, ASCII whitespace, then semver with an optional
/// prerelease tail.
#[must_use = "version results must gate admission"]
pub(crate) fn parse_grok_version(output: &str) -> Option<String> {
    let bytes = output.as_bytes();
    let mut offset = 0_usize;
    while offset + 4 <= bytes.len() {
        let candidate = &bytes[offset..offset + 4];
        let is_grok = candidate[0].eq_ignore_ascii_case(&b'g')
            && candidate[1].eq_ignore_ascii_case(&b'r')
            && candidate[2].eq_ignore_ascii_case(&b'o')
            && candidate[3].eq_ignore_ascii_case(&b'k');
        if is_grok && (offset == 0 || !bytes[offset - 1].is_ascii_alphanumeric()) {
            let spaces = bytes[offset + 4..]
                .iter()
                .take_while(|byte| byte.is_ascii_whitespace())
                .count();
            if spaces > 0
                && let Some(version) = parse_semver(&bytes[offset + 4 + spaces..])
            {
                return Some(version);
            }
        }
        offset += 1;
    }
    None
}

fn take_digits(bytes: &[u8], min: usize, max: usize) -> Option<usize> {
    let count = bytes
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count()
        .min(max);
    if count < min {
        return None;
    }
    Some(count)
}

fn cursor_dated_len(bytes: &[u8]) -> Option<usize> {
    let mut cursor = take_digits(bytes, 4, 4)?;
    if bytes.get(cursor) != Some(&b'.') {
        return None;
    }
    cursor += 1;
    cursor += take_digits(&bytes[cursor..], 1, 2)?;
    if bytes.get(cursor) != Some(&b'.') {
        return None;
    }
    cursor += 1;
    cursor += take_digits(&bytes[cursor..], 1, 2)?;
    if bytes.get(cursor) != Some(&b'-') {
        return None;
    }
    cursor += 1;
    let suffix = bytes[cursor..]
        .iter()
        .take_while(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        .count();
    if suffix == 0 {
        return None;
    }
    Some(cursor + suffix)
}

/// Parses cursor versions, mirroring
/// `/\b(\d{4}\.\d{1,2}\.\d{1,2}-[0-9A-Za-z._-]+)\b/`: a dated release with a
/// mandatory suffix tail.
#[must_use = "version results must gate admission"]
pub(crate) fn parse_cursor_version(output: &str) -> Option<String> {
    let bytes = output.as_bytes();
    let mut offset = 0_usize;
    while offset < bytes.len() {
        let boundary = offset == 0
            || !(bytes[offset - 1].is_ascii_alphanumeric() || bytes[offset - 1] == b'_');
        if boundary && let Some(len) = cursor_dated_len(&bytes[offset..]) {
            return std::str::from_utf8(&bytes[offset..offset + len])
                .ok()
                .map(str::to_owned);
        }
        offset += 1;
    }
    None
}

/// Selects the grok auth method, mirroring the TypeScript evidence: an API
/// key wins when offered, otherwise the cached token, otherwise nothing.
#[must_use = "auth selection must gate the handshake"]
pub(crate) fn grok_select_auth_method(
    available: &[&str],
    has_api_key: bool,
) -> Option<&'static str> {
    if has_api_key && available.contains(&"xai.api_key") {
        return Some("xai.api_key");
    }
    if available.contains(&"cached_token") {
        return Some("cached_token");
    }
    None
}

/// Selects the cursor auth method, mirroring the TypeScript evidence:
/// `cursor_login` when offered, otherwise nothing.
#[must_use = "auth selection must gate the handshake"]
pub(crate) fn cursor_select_auth_method(
    available: &[&str],
    _has_api_key: bool,
) -> Option<&'static str> {
    if available.contains(&"cursor_login") {
        return Some("cursor_login");
    }
    None
}

/// One per-engine ACP definition row: plain data only. No tasks, no I/O,
/// no bridges; the dispatch arms interpret these rows through this core.
#[derive(Clone, Copy, Debug)]
pub(crate) struct AcpDefinition {
    pub(crate) engine_id: &'static str,
    pub(crate) executable: &'static str,
    pub(crate) version_args: &'static [&'static str],
    pub(crate) parse_version: fn(&str) -> Option<String>,
    pub(crate) auth_probe_args: &'static [&'static str],
    pub(crate) is_authenticated_output: fn(&str) -> bool,
    pub(crate) select_auth_method: fn(&[&str], bool) -> Option<&'static str>,
    pub(crate) image_mode: ImageMode,
    pub(crate) build_args: fn(&LaunchArgs) -> Vec<OsString>,
}

/// Grok Build row: `grok` executable, embedded image blocks.
pub(crate) const GROK_ACP: AcpDefinition = AcpDefinition {
    engine_id: "grok",
    executable: "grok",
    version_args: &["--version"],
    parse_version: parse_grok_version,
    auth_probe_args: &["--no-auto-update", "models"],
    is_authenticated_output: default_is_authenticated_output,
    select_auth_method: grok_select_auth_method,
    image_mode: ImageMode::Embedded,
    build_args: grok_build_args,
};

/// Cursor executable name, mirroring the TypeScript platform evidence.
pub(crate) const CURSOR_EXECUTABLE: &str = if cfg!(windows) { "agent.cmd" } else { "agent" };

/// Cursor row: platform executable, native image blocks.
pub(crate) const CURSOR_ACP: AcpDefinition = AcpDefinition {
    engine_id: "cursor",
    executable: CURSOR_EXECUTABLE,
    version_args: &["--version"],
    parse_version: parse_cursor_version,
    auth_probe_args: &["status"],
    is_authenticated_output: default_is_authenticated_output,
    select_auth_method: cursor_select_auth_method,
    image_mode: ImageMode::Image,
    build_args: cursor_build_args,
};
