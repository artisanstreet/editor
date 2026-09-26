//! Sanitized, bounded start diagnostics.
//!
//! An engine child that exits or refuses its session before announcing it
//! usually says why on stderr. The owner retains a bounded tail of that
//! stream only while the child is starting (see [`super::StderrCounter`]);
//! when the start fails, this module reduces the tail to one short,
//! reader-safe line: control and ANSI sequences removed, whitespace
//! collapsed, credential-shaped values redacted, home directories replaced
//! by `~`, and the result capped. Nothing else of the stream ever leaves the
//! owner, and nothing is retained for runs that start.

use std::io;

/// Maximum characters of one surfaced diagnostic, ellipsis included.
pub(crate) const MAX_DIAGNOSTIC_CHARS: usize = 300;
/// Minimum length of an unlabelled base64/hex run treated as a secret.
const MIN_SECRET_RUN: usize = 32;
/// Replacement for every redacted value.
const REDACTED: &str = "[redacted]";
/// Key fragments whose `key=value` / `key: value` values are redacted.
const SENSITIVE_KEYS: [&str; 7] = [
    "token",
    "key",
    "secret",
    "password",
    "passwd",
    "credential",
    "auth",
];
/// Well-known credential prefixes redacted wherever they start a word.
const SECRET_PREFIXES: [&str; 9] = [
    "sk-",
    "sk_",
    "rk_",
    "ghp_",
    "gho_",
    "github_pat_",
    "xoxb-",
    "xoxp-",
    "AKIA",
];

/// One sanitized, bounded, reader-facing reason an engine failed to start.
///
/// Constructed only through the sanitizer (or a fixed typed reason), so the
/// text is always single-line, at most [`MAX_DIAGNOSTIC_CHARS`] characters,
/// and free of control characters, credential-shaped values, and home paths.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StartDiagnostic(String);

impl StartDiagnostic {
    /// Reduces a retained stderr tail to its most informative safe line.
    ///
    /// Prefers the last line labelled as an error (`Error:`, `error:`,
    /// `fatal`, `panic`), else the last non-empty line that is not a stack
    /// frame. The label itself is dropped. Returns `None` when nothing
    /// informative remains.
    pub(crate) fn from_stderr_tail(tail: &[u8]) -> Option<Self> {
        let text = strip_controls(&String::from_utf8_lossy(tail));
        let lines: Vec<&str> = text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect();
        let chosen = lines
            .iter()
            .rev()
            .find(|line| is_error_line(line))
            .or_else(|| lines.iter().rev().find(|line| !is_stack_frame(line)))?;
        Self::sanitize_line(strip_error_label(chosen))
    }

    /// Re-validates text that crossed a seam as a plain string.
    pub(crate) fn from_text(text: &str) -> Option<Self> {
        Self::from_stderr_tail(text.as_bytes())
    }

    /// Typed reason for a spawn failure, when the cause is specific.
    pub(crate) fn for_spawn_error(error: &io::Error) -> Option<Self> {
        let reason = match SpawnFailureKind::classify(error) {
            SpawnFailureKind::ExecutableMissing => "its executable was not found",
            SpawnFailureKind::PermissionDenied => "its executable is not permitted to run",
            SpawnFailureKind::LaunchRejected => "its installation failed verification",
            SpawnFailureKind::Other => return None,
        };
        Some(Self(reason.to_owned()))
    }

    fn sanitize_line(line: &str) -> Option<Self> {
        let redacted = redact_secrets(&redact_home_paths(line));
        let collapsed = redacted.split_whitespace().collect::<Vec<_>>().join(" ");
        if collapsed.is_empty() {
            return None;
        }
        Some(Self(cap_chars(&collapsed)))
    }

    /// The sanitized text.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the diagnostic into its sanitized text.
    pub(crate) fn into_string(self) -> String {
        self.0
    }
}

/// Why spawning an engine child failed, classified without the OS message.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SpawnFailureKind {
    /// The executable does not exist.
    ExecutableMissing,
    /// The operating system refused to execute it.
    PermissionDenied,
    /// The verified launch capability rejected the installation.
    LaunchRejected,
    /// Any other spawn failure.
    Other,
}

impl SpawnFailureKind {
    /// Classifies one raw spawn failure.
    pub(crate) fn classify(error: &io::Error) -> Self {
        if error
            .get_ref()
            .and_then(|inner| inner.downcast_ref::<LaunchRejected>())
            .is_some()
        {
            return Self::LaunchRejected;
        }
        match error.kind() {
            io::ErrorKind::NotFound => Self::ExecutableMissing,
            io::ErrorKind::PermissionDenied => Self::PermissionDenied,
            _ => Self::Other,
        }
    }
}

/// Marker carried by spawn errors whose verified launch was rejected.
#[derive(Debug)]
pub(crate) struct LaunchRejected(pub(crate) &'static str);

impl std::fmt::Display for LaunchRejected {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for LaunchRejected {}

impl LaunchRejected {
    /// Wraps the marker in the permission-denied spawn error it replaces.
    pub(crate) fn error(label: &'static str) -> io::Error {
        io::Error::new(io::ErrorKind::PermissionDenied, Self(label))
    }
}

/// Removes ANSI escape sequences and control characters; `\r` and `\n`
/// become line breaks and tabs become spaces.
fn strip_controls(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\u{1b}' => match chars.next() {
                // CSI: parameters then one final byte in `@`..=`~`.
                Some('[') => {
                    for next in chars.by_ref() {
                        if ('@'..='~').contains(&next) {
                            break;
                        }
                    }
                }
                // OSC: terminated by BEL or `ESC \`.
                Some(']') => {
                    while let Some(next) = chars.next() {
                        if next == '\u{7}' {
                            break;
                        }
                        if next == '\u{1b}' {
                            let _ = chars.next_if_eq(&'\\');
                            break;
                        }
                    }
                }
                _ => {}
            },
            '\r' | '\n' => out.push('\n'),
            '\t' => out.push(' '),
            ch if ch.is_control() => {}
            ch => out.push(ch),
        }
    }
    out
}

fn is_error_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    ["error", "fatal", "panic"]
        .iter()
        .any(|label| lower.starts_with(label))
        || lower.contains("panicked at")
}

fn is_stack_frame(line: &str) -> bool {
    line.starts_with("at ") || line.starts_with('^') || line.starts_with("...")
}

/// Drops a leading `Error:`/`fatal:`-style label when text follows it.
fn strip_error_label(line: &str) -> &str {
    let lower = line.to_ascii_lowercase();
    for label in ["fatal error:", "error:", "fatal:", "panic:"] {
        if lower.starts_with(label) {
            let rest = line[label.len()..].trim_start();
            if !rest.is_empty() {
                return rest;
            }
        }
    }
    line
}

/// Replaces absolute home directories (`/home/<user>`, `/Users/<user>`,
/// `C:\Users\<user>`, `/root`) with `~`.
fn redact_home_paths(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut index = 0;
    while index < line.len() {
        if let Some((start, end)) = home_path_at(line, index) {
            // A drive letter already copied belongs to the redacted path.
            let keep = out.len() - (index - start);
            out.truncate(keep);
            out.push('~');
            index = end;
            continue;
        }
        let ch = line[index..].chars().next().unwrap_or(' ');
        out.push(ch);
        index += ch.len_utf8();
    }
    out
}

/// Returns `(start, end)` of a home directory prefix beginning at `index`.
fn home_path_at(line: &str, index: usize) -> Option<(usize, usize)> {
    let rest = &line[index..];
    let separator = rest.chars().next()?;
    if separator != '/' && separator != '\\' {
        return None;
    }
    let after = &rest[1..];
    let lower = after.to_ascii_lowercase();
    let at_boundary = |offset: usize| {
        after[offset..]
            .chars()
            .next()
            .is_none_or(|ch| ch == '/' || ch == '\\' || !is_path_char(ch))
    };
    if lower.starts_with("root") && at_boundary(4) {
        return Some((index, index + 1 + 4));
    }
    let base = ["home", "users"]
        .iter()
        .find(|base| {
            lower.starts_with(**base)
                && after[base.len()..].starts_with(['/', '\\'])
                && after.len() > base.len() + 1
        })?
        .len();
    let user_start = index + 1 + base + 1;
    let user_len = line[user_start..]
        .find(|ch: char| ch == '/' || ch == '\\' || !is_path_char(ch))
        .unwrap_or(line.len() - user_start);
    if user_len == 0 {
        return None;
    }
    let drive = index >= 2
        && line.as_bytes()[index - 1] == b':'
        && line.as_bytes()[index - 2].is_ascii_alphabetic()
        && (index == 2 || !line.as_bytes()[index - 3].is_ascii_alphanumeric());
    let start = if drive { index - 2 } else { index };
    Some((start, user_start + user_len))
}

fn is_path_char(ch: char) -> bool {
    ch.is_alphanumeric() || matches!(ch, '.' | '_' | '-' | '$' | '@' | '+' | '~')
}

/// How the word after a detached `key:` label is treated.
#[derive(Clone, Copy, Eq, PartialEq)]
enum NextWord {
    Keep,
    /// Redact it (after a password/secret/token label or `Bearer`).
    Redact,
    /// Redact it unless it is a plain word (after a `key`/`auth` label).
    RedactUnlessPlain,
}

/// Redacts credential-shaped values word by word.
fn redact_secrets(line: &str) -> String {
    let words: Vec<&str> = line.split_whitespace().collect();
    let mut out: Vec<String> = Vec::with_capacity(words.len());
    let mut next = NextWord::Keep;
    for word in words {
        let bare = word.trim_matches(|ch: char| matches!(ch, '"' | '\'' | ',' | ';' | '(' | ')'));
        let lower = bare.to_ascii_lowercase();
        if lower == "bearer" || lower == "basic" {
            out.push(word.to_owned());
            next = NextWord::Redact;
            continue;
        }
        let plain = bare.chars().all(char::is_alphabetic);
        match std::mem::replace(&mut next, NextWord::Keep) {
            NextWord::Redact => {
                out.push(REDACTED.to_owned());
                continue;
            }
            NextWord::RedactUnlessPlain if !plain => {
                out.push(REDACTED.to_owned());
                continue;
            }
            NextWord::RedactUnlessPlain | NextWord::Keep => {}
        }
        if let Some(key) = lower.strip_suffix(':') {
            next = detached_label(key);
            if next != NextWord::Keep {
                out.push(word.to_owned());
                continue;
            }
        }
        if is_prefixed_secret(bare) || is_jwt_like(bare) {
            out.push(REDACTED.to_owned());
            continue;
        }
        out.push(redact_long_runs(&redact_assignments(&redact_url_userinfo(
            word,
        ))));
    }
    out.join(" ")
}

fn detached_label(key: &str) -> NextWord {
    if ["token", "secret", "password", "passwd", "credential"]
        .iter()
        .any(|fragment| key.contains(fragment))
    {
        NextWord::Redact
    } else if is_sensitive_key(key) {
        NextWord::RedactUnlessPlain
    } else {
        NextWord::Keep
    }
}

/// `scheme://user:pass@host` keeps the scheme and host only.
fn redact_url_userinfo(word: &str) -> String {
    let Some(scheme_end) = word.find("://") else {
        return word.to_owned();
    };
    let authority_start = scheme_end + 3;
    let authority_end = word[authority_start..]
        .find(['/', '?', '#'])
        .map_or(word.len(), |offset| authority_start + offset);
    match word[authority_start..authority_end].rfind('@') {
        Some(at) => format!(
            "{}{REDACTED}{}",
            &word[..authority_start],
            &word[authority_start + at..]
        ),
        None => word.to_owned(),
    }
}

/// `key=value` (anywhere, e.g. query strings) and a leading `key:value`
/// with a sensitive key keep the key only.
fn redact_assignments(word: &str) -> String {
    let is_key_char = |ch: char| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-';
    let is_value_end = |ch: char| matches!(ch, '&' | ',' | ';' | ')');
    let mut out = String::with_capacity(word.len());
    let mut chars = word.char_indices().peekable();
    let mut first_separator = true;
    while let Some((index, ch)) = chars.next() {
        out.push(ch);
        let attached_colon = ch == ':' && first_separator;
        if ch != '=' && !attached_colon {
            continue;
        }
        first_separator = false;
        let key_start = word[..index]
            .rfind(|candidate: char| !is_key_char(candidate))
            .map_or(0, |position| position + 1);
        let key = &word[key_start..index];
        let value = &word[index + ch.len_utf8()..];
        let value_len = value.find(is_value_end).unwrap_or(value.len());
        let value_text = value[..value_len].trim_matches(['"', '\'']);
        if value_text.is_empty() || value.starts_with("//") || !is_sensitive_key(key) {
            continue;
        }
        out.push_str(REDACTED);
        while chars
            .next_if(|(position, _)| *position < index + ch.len_utf8() + value_len)
            .is_some()
        {}
    }
    out
}

fn is_sensitive_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    !lower.is_empty()
        && SENSITIVE_KEYS
            .iter()
            .any(|fragment| lower.contains(fragment))
}

fn is_prefixed_secret(word: &str) -> bool {
    word.len() >= 10
        && SECRET_PREFIXES
            .iter()
            .any(|prefix| word.starts_with(prefix))
}

/// Three dot-separated base64url segments: `eyJ...` headers, or three long
/// segments that cannot be a host name or version.
fn is_jwt_like(word: &str) -> bool {
    let segments: Vec<&str> = word.split('.').collect();
    if segments.len() != 3 {
        return false;
    }
    let base64url = |segment: &&str| {
        !segment.is_empty()
            && segment
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    };
    if !segments.iter().all(base64url) {
        return false;
    }
    (segments[0].starts_with("eyJ") && segments.iter().all(|segment| segment.len() >= 4))
        || segments.iter().all(|segment| segment.len() >= 10)
}

/// Redacts base64/hex runs of at least [`MIN_SECRET_RUN`] characters that
/// mix letters and digits (plain words and paths are left intact).
fn redact_long_runs(word: &str) -> String {
    let is_run_char = |ch: char| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '/' | '=' | '_');
    let mut out = String::with_capacity(word.len());
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut String| {
        let secret = run.len() >= MIN_SECRET_RUN
            && run.chars().any(|ch| ch.is_ascii_digit())
            && run.chars().any(|ch| ch.is_ascii_alphabetic());
        out.push_str(if secret { REDACTED } else { run.as_str() });
        run.clear();
    };
    for ch in word.chars() {
        if is_run_char(ch) {
            run.push(ch);
        } else {
            flush(&mut run, &mut out);
            out.push(ch);
        }
    }
    flush(&mut run, &mut out);
    out
}

fn cap_chars(text: &str) -> String {
    if text.chars().count() <= MAX_DIAGNOSTIC_CHARS {
        return text.to_owned();
    }
    let mut capped: String = text.chars().take(MAX_DIAGNOSTIC_CHARS - 1).collect();
    capped.push('…');
    capped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surfaced(stderr: &str) -> Option<String> {
        StartDiagnostic::from_stderr_tail(stderr.as_bytes()).map(StartDiagnostic::into_string)
    }

    #[test]
    fn sanitizer_table() {
        let cases: &[(&str, &str)] = &[
            // The motivating Claude refusal: label dropped, text kept.
            (
                "Error: Invalid session ID. Must be a valid UUID.\n",
                "Invalid session ID. Must be a valid UUID.",
            ),
            // Error line preferred over trailing noise and stack frames.
            (
                "warming up\nError: something specific\n    at main (/usr/lib/cli.js:1:2)\n    at next\n",
                "something specific",
            ),
            // The last error line wins over an earlier one.
            ("error: first\ninfo: retry\nfatal: second\n", "second"),
            // No error label: the last non-frame line.
            ("starting\nport already in use\n", "port already in use"),
            // ANSI colour and OSC title sequences are stripped.
            (
                "\u{1b}[31mError:\u{1b}[0m \u{1b}]0;title\u{7}bad \u{1b}[1mflag\u{1b}[0m\n",
                "bad flag",
            ),
            // Carriage-return progress output splits into lines.
            ("10%\r50%\rError: disk full\r", "disk full"),
            // Control characters vanish, tabs and runs of spaces collapse.
            ("Error:\tbad\u{7}\u{0}   value\n", "bad value"),
            // OpenAI/Anthropic-style keys.
            (
                "Error: invalid key sk-ant-api03-abcdefghijklmnop rejected",
                "invalid key [redacted] rejected",
            ),
            // Bearer and Authorization headers.
            (
                "Error: request failed Authorization: Bearer abc.def-ghi",
                "request failed Authorization: Bearer [redacted]",
            ),
            // key=value and key: value credentials.
            (
                "Error: token=abc123 password: hunter2 api_key=\"xyz\" ok",
                "token=[redacted] password: [redacted] api_key=[redacted] ok",
            ),
            // JWT-like triples.
            (
                "Error: bad jwt eyJhbGciOi.eyJzdWIiOiIx.c2lnbmF0dXJl",
                "bad jwt [redacted]",
            ),
            // Long hex and base64 runs.
            (
                "Error: hash 0123456789abcdef0123456789abcdef01 and QUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVo0NTY3=",
                "hash [redacted] and [redacted]",
            ),
            // Home directories on every platform become `~`.
            (
                "Error: cannot read /home/sander/.claude/settings.json",
                "cannot read ~/.claude/settings.json",
            ),
            (
                "Error: cannot read /Users/alice/Library/x and C:\\Users\\Bob\\AppData\\y",
                "cannot read ~/Library/x and ~\\AppData\\y",
            ),
            ("Error: open /root/.config failed", "open ~/.config failed"),
            // Host names, versions, and plain paths survive.
            (
                "Error: api.anthropic.com unreachable (v2.1.3) /usr/local/lib/node_modules/",
                "api.anthropic.com unreachable (v2.1.3) /usr/local/lib/node_modules/",
            ),
        ];
        for (stderr, expected) in cases {
            assert_eq!(
                surfaced(stderr).as_deref(),
                Some(*expected),
                "stderr {stderr:?}"
            );
        }
    }

    #[test]
    fn long_lines_are_capped_with_an_ellipsis() {
        let line = format!("Error: {}", "word ".repeat(200));
        let surfaced = surfaced(&line).expect("diagnostic");
        assert_eq!(surfaced.chars().count(), MAX_DIAGNOSTIC_CHARS);
        assert!(surfaced.ends_with('…'));
        assert!(!surfaced.contains('\n'));
    }

    #[test]
    fn secrets_are_redacted_before_the_cap() {
        let line = format!("Error: {} sk-live-0123456789abcdef", "x".repeat(280));
        let surfaced = surfaced(&line).expect("diagnostic");
        assert!(!surfaced.contains("sk-live"));
        assert!(!surfaced.contains("0123456789"));
    }

    #[test]
    fn empty_or_frame_only_tails_surface_nothing() {
        assert_eq!(surfaced(""), None);
        assert_eq!(surfaced("\n \r\n\t\n"), None);
        assert_eq!(surfaced("\u{1b}[0m\n"), None);
        assert_eq!(surfaced("    at a (x.js:1)\n    at b\n"), None);
    }

    #[test]
    fn a_bare_label_is_kept_rather_than_emptied() {
        assert_eq!(surfaced("Error:\n").as_deref(), Some("Error:"));
    }

    #[test]
    fn invalid_utf8_is_replaced_not_rejected() {
        let surfaced = StartDiagnostic::from_stderr_tail(b"Error: bad \xff byte")
            .expect("diagnostic")
            .into_string();
        assert_eq!(surfaced, "bad \u{fffd} byte");
    }

    #[test]
    fn sanitizing_is_idempotent_across_the_seam() {
        let first = surfaced("Error: token=abc /home/u/x sk-abcdefghijk").expect("diagnostic");
        let second = StartDiagnostic::from_text(&first).expect("diagnostic");
        assert_eq!(second.as_str(), first);
    }

    #[test]
    fn spawn_failures_classify_without_the_os_message() {
        let missing = io::Error::new(io::ErrorKind::NotFound, "/home/u/bin/claude missing");
        assert_eq!(
            StartDiagnostic::for_spawn_error(&missing).map(StartDiagnostic::into_string),
            Some("its executable was not found".to_owned())
        );
        let denied = io::Error::from(io::ErrorKind::PermissionDenied);
        assert_eq!(
            SpawnFailureKind::classify(&denied),
            SpawnFailureKind::PermissionDenied
        );
        let rejected = LaunchRejected::error("claude launch rejected");
        assert_eq!(rejected.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(
            StartDiagnostic::for_spawn_error(&rejected).map(StartDiagnostic::into_string),
            Some("its installation failed verification".to_owned())
        );
        let other = io::Error::other("boom");
        assert_eq!(StartDiagnostic::for_spawn_error(&other), None);
    }
}
