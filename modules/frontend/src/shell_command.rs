//! Presentation of shell invocations reported by an engine.
//!
//! This is the Rust equivalent of
//! `modules/frontend/src/lib/conversation/shell-command.ts`. It deliberately
//! recognizes only the small wrapper prefix that the presentation layer knows
//! about. It does not attempt to interpret the command body as a shell.

#[derive(Clone, Copy)]
enum ShellWrapper {
    PowerShell,
    Posix,
    Cmd,
}

impl ShellWrapper {
    fn accepts_flag(self, flag: &str) -> bool {
        match self {
            Self::PowerShell => matches!(flag, "-command" | "-c"),
            Self::Posix => matches!(flag, "-c" | "-lc" | "-ic" | "-lic"),
            Self::Cmd => matches!(flag, "/c" | "/k"),
        }
    }
}

struct Argument<'a> {
    rest: &'a str,
    value: &'a str,
}

/// Presents the command body of a recognized shell invocation.
///
/// The full invocation is first collapsed using the same whitespace set as
/// JavaScript's `/\s+/g` and `trim()`. A quoted executable path is then read as
/// one leading argument. The executable basename is matched case-insensitively
/// after one case-insensitive `.exe` suffix removal. Only these wrappers and
/// flags are recognized:
///
/// - `pwsh` and `powershell`: `-command`, `-c`;
/// - `bash`, `sh`, `zsh`, and `dash`: `-c`, `-lc`, `-ic`, `-lic`;
/// - `cmd`: `/c`, `/k`.
///
/// For a recognized prefix, one matching pair of quotes surrounding the body
/// is removed. Unknown wrappers or flags, and recognized wrappers with an
/// empty body, return the collapsed invocation unchanged. No shell syntax in
/// the body is interpreted.
#[must_use]
pub fn present_shell_command(command: &str) -> String {
    let collapsed = collapse_javascript_whitespace(command);
    let executable = take_argument(&collapsed);
    let name = executable_name(executable.value);
    let Some(wrapper) = wrapper_for(&name) else {
        return collapsed;
    };

    let flag = take_argument(executable.rest);
    let lowered_flag = flag.value.to_lowercase();
    if !wrapper.accepts_flag(&lowered_flag) {
        return collapsed;
    }

    let body = unquote(trim_javascript_whitespace(flag.rest));
    if body.is_empty() {
        return collapsed;
    }
    body.to_owned()
}

fn collapse_javascript_whitespace(input: &str) -> String {
    let mut collapsed = String::with_capacity(input.len());
    let mut pending_space = false;

    for character in input.chars() {
        if is_javascript_whitespace(character) {
            pending_space = true;
            continue;
        }

        if pending_space && !collapsed.is_empty() {
            collapsed.push(' ');
        }
        pending_space = false;
        collapsed.push(character);
    }

    collapsed
}

fn take_argument(input: &str) -> Argument<'_> {
    let text = input.trim_start_matches(is_javascript_whitespace);
    let quote = match text.as_bytes().first().copied() {
        Some(b'"') => Some('"'),
        Some(b'\'') => Some('\''),
        _ => None,
    };

    if let Some(quote) = quote {
        if let Some(offset) = text[1..].find(quote) {
            let close = offset + 1;
            return Argument {
                rest: &text[close + 1..],
                value: &text[1..close],
            };
        }
    }

    let Some((break_at, _)) = text
        .char_indices()
        .find(|&(_, character)| is_javascript_whitespace(character))
    else {
        return Argument {
            rest: "",
            value: text,
        };
    };

    Argument {
        rest: &text[break_at..],
        value: &text[..break_at],
    }
}

fn executable_name(path: &str) -> String {
    let file = path.rfind(|character| matches!(character, '/' | '\\'));
    let file = file.map_or(path, |separator| &path[separator + 1..]);
    let lowered = file.to_lowercase();
    lowered.strip_suffix(".exe").unwrap_or(&lowered).to_owned()
}

fn wrapper_for(name: &str) -> Option<ShellWrapper> {
    match name {
        "pwsh" | "powershell" => Some(ShellWrapper::PowerShell),
        "bash" | "sh" | "zsh" | "dash" => Some(ShellWrapper::Posix),
        "cmd" => Some(ShellWrapper::Cmd),
        _ => None,
    }
}

fn unquote(text: &str) -> &str {
    let Some(first) = text.as_bytes().first().copied() else {
        return text;
    };
    if text.len() <= 1 || !matches!(first, b'"' | b'\'') {
        return text;
    }

    let quote = first as char;
    if text.ends_with(quote) {
        &text[1..text.len() - 1]
    } else {
        text
    }
}

fn trim_javascript_whitespace(input: &str) -> &str {
    input.trim_matches(is_javascript_whitespace)
}

/// Returns the ECMAScript whitespace and line-terminator characters matched by
/// JavaScript's `\s` regular expression.
///
/// This is intentionally not `char::is_whitespace`: Rust includes U+0085,
/// while ECMAScript includes U+FEFF and does not include U+0085. Keeping the
/// set explicit preserves the source parser's collapse and trim behavior.
fn is_javascript_whitespace(character: char) -> bool {
    matches!(
        character,
        '\u{0009}'
            | '\u{000a}'
            | '\u{000b}'
            | '\u{000c}'
            | '\u{000d}'
            | '\u{0020}'
            | '\u{00a0}'
            | '\u{1680}'
            | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}
