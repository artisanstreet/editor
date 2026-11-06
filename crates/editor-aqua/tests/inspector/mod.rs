use editor_aqua::inspector::{StreamState, command_lines, shell_prompt};

#[test]
fn stream_resets_on_epoch_change_shortening_and_authoritative_snapshot() {
    let mut state = StreamState::default();
    let first = state.update(5, Some(1));
    assert!(first.reset);
    assert_eq!(first.bytes, 0..5);
    let append = state.update(9, Some(1));
    assert!(!append.reset);
    assert_eq!(append.bytes, 5..9);
    assert_eq!(state.update(3, Some(1)).bytes, 0..3);
    assert!(state.update(4, Some(2)).reset);
    assert!(state.update(4, None).reset);
    assert!(state.update(4, None).reset);
}

#[test]
fn shell_command_lines_and_prompts_preserve_platform_conventions() {
    let command = "one\r\ntwo\n";
    let lines = command_lines(command);
    assert_eq!(
        lines
            .iter()
            .map(|range| &command[range.clone()])
            .collect::<Vec<_>>(),
        ["one", "two"]
    );
    assert_eq!(
        shell_prompt(Some("/home/user/project/")).prompt,
        "~/project$ "
    );
    let windows = shell_prompt(Some("C:\\work\\"));
    assert_eq!(windows.prompt, "PS C:\\work> ");
    assert_eq!(windows.continuation, ">> ");
    assert!(!windows.coloured);
}

#[test]
fn terminal_list_updates_dirty_rows_and_evicted_head_without_resetting_scroll() {
    use editor_aqua::inspector::{Layout, ListChange, list_changes};
    let shown = Layout {
        commands: 1,
        lead: 2,
        lines: 3,
        truncated: 4,
        cleared: 0,
    };
    let next = Layout {
        lines: 4,
        truncated: 5,
        ..shown
    };
    assert_eq!(
        list_changes(shown, next, 6, &[1, 3]),
        vec![
            ListChange::Splice {
                range: 5..6,
                count: 0
            },
            ListChange::Splice {
                range: 2..3,
                count: 0
            },
            ListChange::Splice {
                range: 3..4,
                count: 1
            },
            ListChange::Splice {
                range: 4..4,
                count: 3
            },
        ]
    );
    let changed_lead = Layout { lead: 3, ..next };
    assert_eq!(
        list_changes(shown, changed_lead, 6, &[]),
        vec![ListChange::Reset(8)]
    );
}

mod host_lifetime;

#[test]
fn terminal_colors_apply_contrast_inverse_and_dim_in_reference_order() {
    use editor_aqua::{
        inspector::TerminalPalette,
        ui::markdown::terminal::{TerminalColor, TerminalLine, TerminalRun, TerminalStyle},
    };
    let palette = TerminalPalette {
        base: [[0.0, 0.0, 0.0, 1.0]; 16],
        dim: [0.0, 0.0, 0.5, 1.0],
    };
    let line = TerminalLine {
        text: "abc".into(),
        runs: vec![
            TerminalRun {
                range: 0..1,
                style: TerminalStyle {
                    fg: Some(TerminalColor::Rgb(0, 0, 0)),
                    ..TerminalStyle::default()
                },
            },
            TerminalRun {
                range: 1..2,
                style: TerminalStyle {
                    fg: Some(TerminalColor::Rgb(255, 0, 0)),
                    inverse: true,
                    ..TerminalStyle::default()
                },
            },
            TerminalRun {
                range: 2..3,
                style: TerminalStyle {
                    fg: Some(TerminalColor::Rgb(255, 0, 0)),
                    dim: true,
                    ..TerminalStyle::default()
                },
            },
        ],
    };
    let white = [0.0, 0.0, 1.0, 1.0];
    let spans = palette.spans(
        &line,
        &aqua_text::TextStyle::default(),
        white,
        [0.0, 0.0, 0.0, 0.0],
    );
    assert_eq!(spans[0].source_color_hsla, Some(white));
    assert_eq!(spans[1].source_color_hsla, Some([0.0, 0.0, 0.0, 0.0]));
    assert!(spans[1].background_hsla.is_some());
    assert!((spans[2].source_color_hsla.unwrap()[3] - 0.6).abs() < 0.0001);
}
