//! Pure terminal output and session presentation.
//!
//! This is the dependency-free native counterpart of
//! `lib/terminal/presentation.ts`. It owns no PTY, process, transport, or
//! rendering behavior. A caller supplies raw text and already-decoded session
//! metadata; this module only derives the text and small state decisions that
//! a terminal card can present.

/// The lifecycle states a terminal card can receive.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TerminalState {
    /// The session is being opened and is still visible.
    Opening,
    /// The session is running and is still visible.
    Active,
    /// The session has exited normally or been closed.
    Closed,
    /// The session failed and is no longer live.
    Failed,
}

/// The session fields needed by terminal presentation.
///
/// The full protocol session also carries dimensions, ownership, timestamps,
/// and exit metadata. Those concerns deliberately stay outside this pure
/// presentation seam; the fields here are the identity and command facts the
/// TypeScript helpers read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalSession {
    /// Stable identity used to replace a lifecycle update in place.
    pub terminal_id: String,
    /// Executable or path of the interpreter shown as the session title.
    pub executable: String,
    /// Arguments displayed as the command line.
    pub args: Vec<String>,
    /// Current lifecycle state.
    pub state: TerminalState,
}

impl TerminalSession {
    /// Builds a presentation session from an identity, executable, arguments,
    /// and lifecycle state.
    #[must_use]
    pub fn new<I, A>(
        terminal_id: impl Into<String>,
        executable: impl Into<String>,
        args: I,
        state: TerminalState,
    ) -> Self
    where
        I: IntoIterator<Item = A>,
        A: Into<String>,
    {
        Self {
            terminal_id: terminal_id.into(),
            executable: executable.into(),
            args: args.into_iter().map(Into::into).collect(),
            state,
        }
    }
}

/// Maximum number of Unicode scalar values retained by the raw output tail.
pub const OUTPUT_LIMIT_CHARS: usize = 200_000;

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;

/// Appends a raw output chunk and retains only the last bounded character
/// window.
///
/// The bytes are deliberately not sanitized here. Escape sequences may be
/// split across chunks, so [`present_terminal_output`] must see the complete
/// accumulated buffer. The truncation boundary is always a UTF-8 character
/// boundary.
#[must_use]
pub fn append_terminal_output(existing: &str, chunk: &str) -> String {
    let mut combined = String::with_capacity(existing.len().saturating_add(chunk.len()));
    combined.push_str(existing);
    combined.push_str(chunk);

    let character_count = combined.chars().count();
    if character_count <= OUTPUT_LIMIT_CHARS {
        return combined;
    }

    let first_retained = character_count - OUTPUT_LIMIT_CHARS;
    let byte_index = combined
        .char_indices()
        .nth(first_retained)
        .map_or(combined.len(), |(index, _)| index);
    combined[byte_index..].to_owned()
}

/// Presents accumulated terminal output as readable plain text.
///
/// Recognized ANSI sequences and C0 control bytes are removed first. Each
/// resulting line then applies terminal-style carriage-return overwrite, so
/// progress updates occupy one visible line instead of stacking. Sanitizing
/// the complete input here is what keeps split escape sequences correct.
#[must_use]
pub fn present_terminal_output(raw: &str) -> String {
    let plain = strip_terminal_sequences(raw);
    plain
        .split('\n')
        .map(resolve_carriage_returns)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Removes the sequence classes covered by the TypeScript ANSI expression,
/// followed by the same explicit C0/DEL control-byte set.
fn strip_terminal_sequences(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut plain = String::with_capacity(raw.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == ESC {
            if let Some(end) = ansi_sequence_end(bytes, index) {
                index = end;
            } else {
                // ESC itself is in the control pattern. If the following
                // bytes do not form a recognized sequence, drop only ESC and
                // let the remaining printable bytes be considered normally.
                index += 1;
            }
            continue;
        }

        if is_removed_control(bytes[index]) {
            index += 1;
            continue;
        }

        // `raw` is valid UTF-8 and `index` is maintained at a character
        // boundary, so this always advances by one complete scalar value.
        let character = raw[index..]
            .chars()
            .next()
            .expect("a non-empty UTF-8 suffix has a first character");
        plain.push(character);
        index += character.len_utf8();
    }

    plain
}

/// Returns the end of a recognized ANSI sequence beginning at `start`.
///
/// This mirrors the four alternatives in the TypeScript expression:
/// 7-bit CSI, OSC, DCS/SOS/PM/APC, and the listed two-byte escapes. An
/// unterminated string sequence consumes the remainder, just as the optional
/// terminator in the original expression permits.
fn ansi_sequence_end(bytes: &[u8], start: usize) -> Option<usize> {
    let kind = *bytes.get(start + 1)?;
    match kind {
        b'[' => csi_sequence_end(bytes, start),
        b']' => Some(osc_sequence_end(bytes, start)),
        b'P' | b'X' | b'^' | b'_' => Some(string_sequence_end(bytes, start)),
        // `[ @-Z\\]^_ ]`: the same two-character class as the source regex.
        // The string-sequence starters above take precedence for `]`, `^`,
        // and `_`; only the remaining members are listed here.
        b'@'..=b'Z' | b'\\' => Some(start + 2),
        _ => None,
    }
}

/// Returns the end of a CSI sequence, if its parameter/intermediate/final
/// bytes have the shape accepted by the source expression.
fn csi_sequence_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut index = start + 2;

    while let Some(byte) = bytes.get(index) {
        if (0x30..=0x3f).contains(byte) {
            index += 1;
        } else {
            break;
        }
    }
    while let Some(byte) = bytes.get(index) {
        if (0x20..=0x2f).contains(byte) {
            index += 1;
        } else {
            break;
        }
    }

    match bytes.get(index) {
        Some(0x40..=0x7e) => Some(index + 1),
        _ => None,
    }
}

/// Returns the end of an OSC, DCS, SOS, PM, or APC string sequence.
fn string_sequence_end(bytes: &[u8], start: usize) -> usize {
    let mut index = start + 2;
    while index < bytes.len() {
        match bytes[index] {
            ESC if bytes.get(index + 1) == Some(&b'\\') => return index + 2,
            ESC => return index,
            _ => index += 1,
        }
    }
    bytes.len()
}

/// Returns the end of an OSC string, whose BEL terminator is distinct from
/// the other control-string classes where BEL is ordinary payload.
fn osc_sequence_end(bytes: &[u8], start: usize) -> usize {
    let mut index = start + 2;
    while index < bytes.len() {
        match bytes[index] {
            BEL => return index + 1,
            ESC if bytes.get(index + 1) == Some(&b'\\') => return index + 2,
            ESC => return index,
            _ => index += 1,
        }
    }
    bytes.len()
}

/// The C0 and DEL bytes removed by the source control expression.
const fn is_removed_control(byte: u8) -> bool {
    matches!(byte, 0x00..=0x08 | 0x0b | 0x0c | 0x0e..=0x1f | 0x7f)
}

/// Resolves carriage returns as a terminal cursor return to column zero.
fn resolve_carriage_returns(line: &str) -> String {
    let mut resolved = String::new();

    for (segment_index, segment) in line.split('\r').enumerate() {
        if segment_index == 0 {
            resolved.push_str(segment);
            continue;
        }

        let segment_characters = segment.chars().count();
        let resolved_characters = resolved.chars().count();
        if segment_characters >= resolved_characters {
            resolved.clear();
            resolved.push_str(segment);
        } else {
            let suffix = resolved
                .char_indices()
                .nth(segment_characters)
                .map_or("", |(index, _)| &resolved[index..]);
            let mut overwritten = String::with_capacity(segment.len() + suffix.len());
            overwritten.push_str(segment);
            overwritten.push_str(suffix);
            resolved = overwritten;
        }
    }

    resolved
}

/// Returns the command arguments in their compact terminal-card form.
///
/// The source presentation quotes only arguments containing a literal space;
/// it does not shell-escape quotes or treat other whitespace specially.
#[must_use]
pub fn terminal_command_line(session: &TerminalSession) -> String {
    let mut line = String::new();

    for (index, argument) in session.args.iter().enumerate() {
        if index != 0 {
            line.push(' ');
        }
        if argument.contains(' ') {
            line.push('"');
            line.push_str(argument);
            line.push('"');
        } else {
            line.push_str(argument);
        }
    }

    line
}

/// Returns the friendly display name for a terminal's interpreter.
///
/// Both slash styles are path separators, and only the executable suffixes
/// recognized by the source are removed. Unknown programs retain their bare
/// basename rather than being guessed as a generic shell.
#[must_use]
pub fn terminal_display_name(session: &TerminalSession) -> String {
    let program = session
        .executable
        .rsplit(|character| matches!(character, '/' | '\\'))
        .next()
        .unwrap_or(session.executable.as_str());
    let bare = strip_executable_suffix(program);

    shell_display_name(bare)
        .map(str::to_owned)
        .unwrap_or_else(|| bare.to_owned())
}

/// Removes one case-insensitive executable suffix from the basename.
fn strip_executable_suffix(program: &str) -> &str {
    for suffix in [".bat", ".cmd", ".exe", ".ps1"] {
        if program.len() >= suffix.len()
            && program[program.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
        {
            return &program[..program.len() - suffix.len()];
        }
    }
    program
}

/// Exact friendly shell names carried by the TypeScript presentation map.
fn shell_display_name(program: &str) -> Option<&'static str> {
    const NAMES: [(&str, &str); 15] = [
        ("ash", "Ash"),
        ("bash", "Bash"),
        ("cmd", "Command Prompt"),
        ("dash", "Dash"),
        ("elvish", "Elvish"),
        ("fish", "Fish"),
        ("ksh", "Ksh"),
        ("nu", "Nushell"),
        ("nushell", "Nushell"),
        ("powershell", "PowerShell"),
        ("pwsh", "PowerShell"),
        ("sh", "Shell"),
        ("shell", "Shell"),
        ("xonsh", "Xonsh"),
        ("zsh", "Zsh"),
    ];

    NAMES
        .iter()
        .find(|(name, _)| program.eq_ignore_ascii_case(name))
        .map(|(_, display)| *display)
}

/// Returns whether a terminal remains visible while it is opening or active.
#[must_use]
pub const fn is_live_terminal(session: &TerminalSession) -> bool {
    matches!(
        session.state,
        TerminalState::Opening | TerminalState::Active
    )
}

/// Applies one lifecycle update, replacing an existing identity in its
/// original position or appending a previously unseen identity.
#[must_use]
pub fn apply_terminal_lifecycle(
    terminals: &[TerminalSession],
    next: &TerminalSession,
) -> Vec<TerminalSession> {
    let mut updated = terminals.to_vec();
    let mut replaced = false;
    for existing in &mut updated {
        if existing.terminal_id == next.terminal_id {
            *existing = next.clone();
            replaced = true;
        }
    }
    if !replaced {
        updated.push(next.clone());
    }
    updated
}
