//! Focused dependency-free coverage for terminal presentation.
//!
//! The source is loaded directly so this packet does not require shared
//! frontend exports or BUILD/Cargo registration.

#[path = "../../modules/frontend/src/terminal_presentation.rs"]
mod terminal_presentation;

use terminal_presentation::{
    OUTPUT_LIMIT_CHARS, TerminalSession, TerminalState, append_terminal_output,
    apply_terminal_lifecycle, is_live_terminal, present_terminal_output, terminal_command_line,
    terminal_display_name,
};

fn session(
    terminal_id: &str,
    executable: &str,
    args: &[&str],
    state: TerminalState,
) -> TerminalSession {
    TerminalSession::new(terminal_id, executable, args.iter().copied(), state)
}

#[test]
fn raw_tail_is_bounded_by_unicode_characters_without_splitting_utf8() {
    let existing = "x".repeat(OUTPUT_LIMIT_CHARS - 1);
    let retained = append_terminal_output(&existing, "🙂z");

    assert_eq!(retained.chars().count(), OUTPUT_LIMIT_CHARS);
    assert_eq!(
        retained,
        format!("{}🙂z", "x".repeat(OUTPUT_LIMIT_CHARS - 2))
    );
    assert!(std::str::from_utf8(retained.as_bytes()).is_ok());

    let already_bounded = append_terminal_output("🙂", "");
    assert_eq!(already_bounded, "🙂");
}

#[test]
fn raw_chunks_remain_unsanitized_until_the_accumulated_buffer_is_presented() {
    let chunks = ["prefix\u{1b}[3", "1mred\u{1b}]0;ti", "tle", "\u{07}suffix"];
    let mut raw = String::new();
    for chunk in chunks {
        raw = append_terminal_output(&raw, chunk);
    }

    assert_eq!(raw, "prefix\u{1b}[31mred\u{1b}]0;title\u{07}suffix");
    assert_eq!(present_terminal_output(&raw), "prefixredsuffix");
}

#[test]
fn split_control_strings_are_removed_after_their_later_terminators_arrive() {
    let chunks = [
        "a\u{1b}Pdc",
        "s payload\u{07}\u{1b}",
        "\\b\u{1b}^pm",
        " payload\u{1b}",
        "\\c\u{1b}_apc",
        " payload\u{1b}",
        "\\d",
    ];
    let mut raw = String::new();
    for chunk in chunks {
        raw = append_terminal_output(&raw, chunk);
    }

    assert_eq!(present_terminal_output(&raw), "abcd");
}

#[test]
fn all_supported_ansi_string_classes_and_two_character_escapes_are_removed() {
    let raw = concat!(
        "before",
        "\u{1b}[38;5;196m",
        "\u{1b}]0;window title\u{07}",
        "\u{1b}Pdcs\u{07} payload\u{1b}\\",
        "\u{1b}Xsos payload\u{1b}\\",
        "\u{1b}^pm payload\u{1b}\\",
        "\u{1b}_apc payload\u{1b}\\",
        "\u{1b}M",
        "after",
    );

    assert_eq!(present_terminal_output(raw), "beforeafter");
}

#[test]
fn malformed_or_unknown_escape_bytes_follow_the_control_filter() {
    // ESC is removed even when it is not followed by a recognized sequence;
    // the printable suffix remains. Tabs, newlines, and carriage returns are
    // intentionally not members of the source control range.
    let raw = "a\u{1b}q[unterminated\tb\u{7f}\u{0b}c\nline\rnext";
    assert_eq!(present_terminal_output(raw), "aq[unterminated\tbc\nnext");
}

#[test]
fn carriage_returns_overwrite_progress_frames_and_preserve_uncovered_columns() {
    assert_eq!(
        present_terminal_output("Downloading 10%\rDownloading 100%"),
        "Downloading 100%"
    );
    assert_eq!(present_terminal_output("abc\rxy"), "xyc");
    assert_eq!(present_terminal_output("abc\rxy\rz"), "zyc");
    assert_eq!(present_terminal_output("猫猫猫\r犬"), "犬猫猫");
    assert_eq!(
        present_terminal_output("one\rtwo\nthree\rfour"),
        "two\nfoure"
    );
}

#[test]
fn command_line_quotes_only_arguments_containing_literal_spaces() {
    let command = session(
        "terminal-1",
        "bash",
        &["npm", "run dev", "--name=two words", "tab\tvalue"],
        TerminalState::Active,
    );

    assert_eq!(
        terminal_command_line(&command),
        "npm \"run dev\" \"--name=two words\" tab\tvalue"
    );
    assert_eq!(
        terminal_command_line(&session("empty", "sh", &[], TerminalState::Opening)),
        ""
    );
}

#[test]
fn executable_paths_suffixes_and_friendly_shell_names_match_exactly() {
    let cases = [
        ("C:\\Program Files\\PowerShell\\pwsh.EXE", "PowerShell"),
        ("/usr/local/bin/BASH", "Bash"),
        ("C:/Windows/System32/cmd.CMD", "Command Prompt"),
        ("/bin/dash", "Dash"),
        ("/bin/fish.exe", "Fish"),
        ("/opt/nu.ps1", "Nushell"),
        ("/opt/NuShell.BAT", "Nushell"),
        ("/opt/xonsh", "Xonsh"),
        ("/opt/Oil.EXE", "Oil"),
        ("/opt/tool.exe.old", "tool.exe.old"),
    ];

    for (executable, expected) in cases {
        let terminal = session("terminal-name", executable, &[], TerminalState::Closed);
        assert_eq!(terminal_display_name(&terminal), expected, "{executable}");
    }
}

#[test]
fn every_shell_alias_uses_its_exact_friendly_title() {
    let cases = [
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

    for (executable, expected) in cases {
        let terminal = session("terminal-name", executable, &[], TerminalState::Active);
        assert_eq!(terminal_display_name(&terminal), expected);
    }
}

#[test]
fn only_opening_and_active_states_are_live() {
    for (state, expected) in [
        (TerminalState::Opening, true),
        (TerminalState::Active, true),
        (TerminalState::Closed, false),
        (TerminalState::Failed, false),
    ] {
        let terminal = session("terminal-state", "sh", &[], state);
        assert_eq!(is_live_terminal(&terminal), expected, "{state:?}");
    }
}

#[test]
fn lifecycle_updates_replace_in_place_and_preserve_order() {
    let first = session("first", "bash", &["one"], TerminalState::Opening);
    let second = session("second", "zsh", &["two"], TerminalState::Active);
    let replacement = session("first", "bash", &["done"], TerminalState::Closed);

    let updated = apply_terminal_lifecycle(&[first, second], &replacement);

    assert_eq!(updated.len(), 2);
    assert_eq!(
        updated
            .iter()
            .map(|terminal| terminal.terminal_id.as_str())
            .collect::<Vec<_>>(),
        ["first", "second"]
    );
    assert_eq!(updated[0], replacement);
}

#[test]
fn lifecycle_appends_a_new_identity_after_existing_sessions() {
    let first = session("first", "bash", &[], TerminalState::Active);
    let second = session("second", "pwsh.exe", &["run dev"], TerminalState::Opening);

    let updated = apply_terminal_lifecycle(std::slice::from_ref(&first), &second);

    assert_eq!(updated, vec![first, second]);
}
