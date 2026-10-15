//! Terminal model and paint mapping over real-world-shaped captures.

use std::sync::Arc;

use gpui::{FontWeight, HighlightStyle, hsla};

use super::{
    TerminalColor, TerminalLine, TerminalPalette, TerminalScreen, TerminalStyle, highlights,
};
use crate::theme::ThemeMode;

/// Vite 5 startup banner, including its leading clear screen.
const VITE_BANNER: &str = "\x1b[2J\x1b[3J\x1b[H\n  \x1b[32m\x1b[1mVITE\x1b[22m v5.4.2\x1b[39m  \
\x1b[2mready in \x1b[0m\x1b[1m312\x1b[22m\x1b[2m\x1b[0m ms\x1b[22m\n\n  \x1b[32m➜\x1b[39m  \
\x1b[1mLocal\x1b[22m:   \x1b[36mhttp://localhost:\x1b[1m5173\x1b[22m/\x1b[39m\n";

/// A Vite transform error.
const VITE_ERROR: &str = "\x1b[2m10:42:01 PM\x1b[22m \x1b[31m\x1b[1m[vite]\x1b[22m \
\x1b[31mInternal server error: Failed to resolve import \"./missing\"\x1b[39m\n  \
Plugin: \x1b[35mvite:import-analysis\x1b[39m\n";

/// A rustc diagnostic header as cargo prints it with colour forced.
const CARGO_ERROR: &str = "\x1b[0m\x1b[1m\x1b[38;5;9merror[E0308]\x1b[0m\x1b[0m\x1b[1m: \
mismatched types\x1b[0m\n\x1b[0m \x1b[0m\x1b[0m\x1b[1m\x1b[38;5;12m--> \x1b[0m\x1b[0msrc/main.rs:4:18\x1b[0m\n";

/// Every capture plus the awkward cases, for split-feed equivalence.
fn kitchen_sink() -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"old line\n");
    bytes.extend_from_slice(VITE_BANNER.as_bytes());
    bytes.extend_from_slice(VITE_ERROR.as_bytes());
    bytes.extend_from_slice(CARGO_ERROR.as_bytes());
    bytes.extend_from_slice("  Compiling 10%\r  Compiling 70%\r✓ Compiled\n".as_bytes());
    bytes.extend_from_slice("⠋ build\n\x1b[1A\x1b[2K⠙ build\n\x1b[1A\x1b[2K✓ built\n".as_bytes());
    bytes.extend_from_slice(b"see \x1b]8;;https://example.com\x1b\\docs\x1b]8;;\x07 now\n");
    bytes.extend_from_slice(b"\x1b[38;5;208mA\x1b[48;2;10;20;30mB\x1b[38:2::1:2:3mC\x1b[0m\n");
    bytes.extend_from_slice(b"\x1bP1$r0m\x1b\\\x1b[?25l\x1b[?2026h\x1b(Bdone\t|\x08!\n");
    bytes
}

fn screen(input: &str) -> TerminalScreen {
    let mut screen = TerminalScreen::new();
    screen.feed(input.as_bytes());
    screen
}

fn texts(screen: &TerminalScreen) -> Vec<&str> {
    screen.lines().map(|line| line.text.as_str()).collect()
}

/// The style covering `needle`'s first occurrence, asserting one run spans it.
fn style_of<'a>(line: &'a TerminalLine, needle: &str) -> &'a TerminalStyle {
    let start = line.text.find(needle).expect("needle in line");
    let range = start..start + needle.len();
    &line
        .runs
        .iter()
        .find(|run| run.range.start <= range.start && range.end <= run.range.end)
        .unwrap_or_else(|| panic!("no single run covers {needle:?} in {line:?}"))
        .style
}

#[test]
fn vite_banner_keeps_colours_and_bold() {
    let screen = screen(VITE_BANNER);
    assert_eq!(
        texts(&screen),
        [
            "",
            "  VITE v5.4.2  ready in 312 ms",
            "",
            "  ➜  Local:   http://localhost:5173/"
        ]
    );
    // The startup clear had nothing to hide.
    assert!(!screen.cleared_earlier());
    let banner = screen.line(1).expect("banner");
    let vite = style_of(banner, "VITE");
    assert_eq!(
        (vite.fg, vite.bold),
        (Some(TerminalColor::Indexed(2)), true)
    );
    let version = style_of(banner, "v5.4.2");
    assert_eq!(
        (version.fg, version.bold),
        (Some(TerminalColor::Indexed(2)), false)
    );
    assert!(style_of(banner, "ready in").dim);
    assert!(style_of(banner, "312").bold);
    let local = screen.line(3).expect("local");
    let port = style_of(local, "5173");
    assert_eq!(
        (port.fg, port.bold),
        (Some(TerminalColor::Indexed(6)), true)
    );
    // Plain text carries no run.
    assert!(local.runs.iter().all(|run| run.range.start >= 2));
}

#[test]
fn vite_error_is_red() {
    let screen = screen(VITE_ERROR);
    let line = screen.line(0).expect("error line");
    assert!(
        line.text
            .ends_with("Failed to resolve import \"./missing\"")
    );
    assert_eq!(
        style_of(line, "Internal server error").fg,
        Some(TerminalColor::Indexed(1))
    );
    let tag = style_of(line, "[vite]");
    assert_eq!((tag.fg, tag.bold), (Some(TerminalColor::Indexed(1)), true));
    assert_eq!(
        style_of(screen.line(1).expect("plugin"), "vite:import-analysis").fg,
        Some(TerminalColor::Indexed(5))
    );
}

#[test]
fn cargo_error_header_is_bold_bright_red() {
    let screen = screen(CARGO_ERROR);
    assert_eq!(
        texts(&screen),
        ["error[E0308]: mismatched types", " --> src/main.rs:4:18"]
    );
    let header = screen.line(0).expect("header");
    let code = style_of(header, "error[E0308]");
    assert_eq!(
        (code.fg, code.bold),
        (Some(TerminalColor::Indexed(9)), true)
    );
    let message = style_of(header, ": mismatched types");
    assert_eq!((message.fg, message.bold), (None, true));
    // Adjacent equal styles merge into one run per span.
    assert_eq!(header.runs.len(), 2);
    assert_eq!(header.runs[0].range, 0.."error[E0308]".len());
}

#[test]
fn carriage_return_overwrites_cell_by_cell() {
    let screen = screen("Downloading  10%\rDownloading  55%\rDownloading 100%\ndone\nabcdef\rXY");
    assert_eq!(texts(&screen), ["Downloading 100%", "done", "XYcdef"]);
}

#[test]
fn spinner_redraws_one_line() {
    let screen = screen("⠋ building\n\x1b[1A\x1b[2K⠙ building\n\x1b[1A\x1b[2K✓ built\n");
    assert_eq!(texts(&screen), ["✓ built"]);
    assert_eq!(screen.last_nonempty_line(), Some("✓ built"));
}

#[test]
fn clear_screen_drops_earlier_lines() {
    let screen = screen("old 1\nold 2\n\x1b[2J\x1b[3J\x1b[Hnew\n");
    assert_eq!(texts(&screen), ["new"]);
    assert!(screen.cleared_earlier());
    assert_eq!(screen.cleared_line_count(), 2);
    let reset = self::screen("a\nb\x1bcfresh");
    assert_eq!(texts(&reset), ["fresh"]);
    assert_eq!(reset.cleared_line_count(), 2);
}

#[test]
fn osc8_links_set_and_clear() {
    let screen = screen("see \x1b]8;id=1;https://example.com\x1b\\docs\x1b]8;;\x1b\\ now");
    let line = screen.line(0).expect("line");
    assert_eq!(line.text, "see docs now");
    assert_eq!(line.runs.len(), 1);
    assert_eq!(line.runs[0].range, 4..8);
    assert_eq!(
        line.runs[0].style.link,
        Some(Arc::<str>::from("https://example.com"))
    );
    // Other OSC (window title) is dropped, BEL-terminated or not.
    assert_eq!(texts(&self::screen("\x1b]0;title\x07x")), ["x"]);
}

#[test]
fn extended_colours_in_both_forms() {
    let screen =
        screen("\x1b[38;5;208mA\x1b[48;2;10;20;30mB\x1b[38:2::1:2:3mC\x1b[38:5:33;4:0mD\x1b[0mE");
    let line = screen.line(0).expect("line");
    assert_eq!(line.text, "ABCDE");
    assert_eq!(style_of(line, "A").fg, Some(TerminalColor::Indexed(208)));
    let b = style_of(line, "B");
    assert_eq!(
        (b.fg, b.bg),
        (
            Some(TerminalColor::Indexed(208)),
            Some(TerminalColor::Rgb(10, 20, 30))
        )
    );
    assert_eq!(style_of(line, "C").fg, Some(TerminalColor::Rgb(1, 2, 3)));
    assert_eq!(style_of(line, "D").fg, Some(TerminalColor::Indexed(33)));
    assert_eq!(line.runs.len(), 4);
}

#[test]
fn sgr_resets_and_attributes() {
    let screen = screen("\x1b[1;2;3;4;7;9mX\x1b[22;23;24;27mY\x1b[29;97;104mZ\x1b[39;49mW");
    let line = screen.line(0).expect("line");
    let x = style_of(line, "X");
    assert!(x.bold && x.dim && x.italic && x.underline && x.inverse && x.strikethrough);
    let y = style_of(line, "Y");
    assert!(y.strikethrough && !y.bold && !y.dim && !y.italic && !y.underline && !y.inverse);
    let z = style_of(line, "Z");
    assert_eq!(
        (z.fg, z.bg),
        (
            Some(TerminalColor::Indexed(15)),
            Some(TerminalColor::Indexed(12))
        )
    );
    assert!(!z.strikethrough);
    assert_eq!(line.runs.len(), 3, "W is plain and carries no run");
}

#[test]
fn split_feeds_match_a_single_feed() {
    let bytes = kitchen_sink();
    let whole = {
        let mut screen = TerminalScreen::new();
        screen.feed(&bytes);
        screen
    };
    let expected: Vec<TerminalLine> = whole.lines().cloned().collect();
    assert!(expected.len() > 5, "{expected:?}");
    assert!(whole.cleared_earlier());
    for split in 0..=bytes.len() {
        let mut screen = TerminalScreen::new();
        screen.feed(&bytes[..split]);
        screen.feed(&bytes[split..]);
        let lines: Vec<TerminalLine> = screen.lines().cloned().collect();
        assert_eq!(lines, expected, "split at {split}");
        assert_eq!(screen.cleared_line_count(), whole.cleared_line_count());
    }
    let mut bytewise = TerminalScreen::new();
    for byte in &bytes {
        bytewise.feed(std::slice::from_ref(byte));
    }
    assert!(bytewise.lines().eq(whole.lines()));
}

#[test]
fn kitchen_sink_drops_unknown_sequences() {
    let mut screen = TerminalScreen::new();
    screen.feed(&kitchen_sink());
    assert_eq!(screen.last_nonempty_line(), Some("done    !"));
    assert!(!screen.plain_text().contains('\x1b'));
}

#[test]
fn invalid_utf8_becomes_replacement() {
    let mut screen = TerminalScreen::new();
    screen.feed(b"a\xffb\xe2\x82");
    screen.feed(b"z\xf0\x9f");
    screen.feed(b"\x98\x80");
    assert_eq!(texts(&screen), ["a\u{FFFD}b\u{FFFD}z😀"]);
}

#[test]
fn trailing_newline_is_not_content() {
    assert_eq!(screen("a\n").line_count(), 1);
    assert_eq!(texts(&screen("a\n\n")), ["a", ""]);
    assert_eq!(screen("").line_count(), 0);
    assert_eq!(screen("a\nb").plain_text(), "a\nb");
    assert_eq!(screen("x\n  \n").last_nonempty_line(), Some("x"));
}

#[test]
fn cursor_moves_tabs_and_erases() {
    assert_eq!(texts(&screen("ab\tc\x08d")), ["ab      d"]);
    assert_eq!(texts(&screen("hello\x1b[3D\x1b[KXY")), ["heXY"]);
    assert_eq!(texts(&screen("hello\x1b[1G\x1b[2CZ")), ["heZlo"]);
    assert_eq!(texts(&screen("hello\x1b[3G\x1b[1K")), ["   lo"]);
    assert_eq!(texts(&screen("1\n2\n3\x1b[2;1H\x1b[J")), ["1"]);
    assert_eq!(
        texts(&screen("1\n2\n3\x1b7\x1b[1;1HX\x1b8Y")),
        ["X", "2", "3Y"]
    );
    assert_eq!(texts(&screen("1\n2\x1b[5A\x1b[9BZ")), ["1", "2Z"]);
}

#[test]
fn random_bytes_never_panic() {
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (state >> 33) as u32
    };
    // Bias toward the bytes that drive the state machine.
    let alphabet =
        b"\x1b\x1b[[]]0123456789;;::mKJHABCDGfsu78c?P\\\x07\r\n\t\x08 ax\xe2\x82\xac\xff";
    let mut screen = TerminalScreen::with_max_lines(64);
    for _ in 0..400 {
        let len = next() % 512;
        let chunk: Vec<u8> = (0..len)
            .map(|_| {
                let roll = next();
                if roll % 4 == 0 {
                    roll.to_le_bytes()[1]
                } else {
                    alphabet[(roll >> 8) as usize % alphabet.len()]
                }
            })
            .collect();
        screen.feed(&chunk);
        assert!(screen.line_count() <= 64);
        let _ = screen.plain_text();
        let _ = screen.last_nonempty_line();
    }
}

#[test]
fn highlights_map_styles() {
    let dark = TerminalPalette::for_mode(ThemeMode::Dark);
    let foreground = hsla(0.0, 0.0, 0.9, 1.0);
    let background = hsla(0.0, 0.0, 0.1, 1.0);
    let screen = screen(
        "\x1b[1;31mE\x1b[0m \x1b[2mD\x1b[0m \x1b[7mI\x1b[0m \x1b[30mK\x1b[0m \x1b[3;9mS\x1b[0m \
         \x1b]8;;https://x.dev\x07L\x1b]8;;\x07 plain",
    );
    let line = screen.line(0).expect("line");
    let runs = highlights(line, &dark, foreground, background);
    let at = |needle: &str| -> HighlightStyle {
        let start = line.text.find(needle).expect("needle");
        runs.iter()
            .find(|(range, _)| range.start == start)
            .map(|(_, style)| *style)
            .expect("highlight")
    };
    let error = at("E");
    assert_eq!(error.font_weight, Some(FontWeight::BOLD));
    assert_eq!(error.color, Some(dark.base[1]));
    let dim = at("D").color.expect("dim colour");
    assert!((dim.alpha - 0.6).abs() < 1e-6);
    let inverse = at("I");
    assert_eq!(
        (inverse.color, inverse.background_color),
        (Some(background), Some(foreground))
    );
    // Black text on the dark background is lifted until it reads.
    let black = at("K").color.expect("black");
    assert!(black.lightness >= 0.25, "{black:?}");
    let struck = at("S");
    assert!(struck.strikethrough.is_some() && struck.font_style.is_some());
    assert!(at("L").underline.is_some());
    assert_eq!(runs.len(), 6);
}

#[test]
fn palette_resolves_cube_and_ramp() {
    let palette = TerminalPalette::for_mode(ThemeMode::Light);
    let red = gpui::hsla_to_rgba(palette.resolve(TerminalColor::Indexed(196)));
    assert!((red.red - 1.0).abs() < 1e-3 && red.green.abs() < 1e-3 && red.blue.abs() < 1e-3);
    let grey = gpui::hsla_to_rgba(palette.resolve(TerminalColor::Indexed(244)));
    assert!((grey.red - 128.0 / 255.0).abs() < 1e-3);
    assert_eq!(
        palette.resolve(TerminalColor::Indexed(4)),
        TerminalPalette::for_mode(ThemeMode::Light).base[4]
    );
}

#[test]
fn wide_character_overwrites_use_display_columns() {
    let mut screen = TerminalScreen::new();
    screen.feed("界a\r\x1b[2Cb".as_bytes());
    assert_eq!(screen.plain_text(), "界b");
}

#[test]
fn combining_marks_do_not_advance_the_terminal_cursor() {
    let mut screen = TerminalScreen::new();
    screen.feed("e\u{301}x\r\x1b[1Cy".as_bytes());
    assert_eq!(screen.plain_text(), "e\u{301}y");
}

#[test]
fn dirty_rows_include_cursor_writes_above_the_tail_and_survive_materialization() {
    let mut screen = TerminalScreen::new();
    screen.feed(b"first\nsecond\nthird");
    assert_eq!(screen.take_dirty_line_indices(), [0, 1, 2]);
    screen.feed(b"\x1b[1;1Hnew");
    let _ = screen.last_nonempty_line();
    assert_eq!(screen.take_dirty_line_indices(), [0]);
    assert!(screen.take_dirty_line_indices().is_empty());
}
