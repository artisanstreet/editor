//! Media kinds: parsing, Markdown and colors.

use super::super::{Prepared, RichFence, prepare};
use super::color::{Color, Notation, parse};

fn ready(kind: &str, json: &str) -> Prepared {
    match prepare(Some(&format!("artisan-{kind}")), json, true) {
        Some(RichFence::Ready(prepared)) => prepared,
        other => panic!("expected artisan-{kind} to parse, got {other:?}"),
    }
}

fn invalid(kind: &str, json: &str) -> String {
    match prepare(Some(&format!("artisan-{kind}")), json, true) {
        Some(RichFence::Invalid { note }) => note.to_string(),
        other => panic!("expected artisan-{kind} to be invalid, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Kinds

#[test]
fn image_compare_needs_both_sides() {
    let markdown = ready(
        "image-compare",
        r#"{"before": {"src": "old.png"}, "after": {"src": "new.png", "label": "Fixed"}}"#,
    )
    .markdown();
    assert!(markdown.contains("**Before**"), "{markdown}");
    assert!(markdown.contains("![Fixed](new.png)"), "{markdown}");
    assert!(invalid("image-compare", r#"{"before": {"src": "a.png"}}"#).contains("after"));
}

#[test]
fn model_takes_a_src_and_reads_as_a_link() {
    let markdown = ready("model", r#"{"src": "assets/robot arm.fbx"}"#).markdown();
    assert_eq!(markdown.as_ref(), "[robot arm.fbx](assets/robot%20arm.fbx)");
    let named = ready(
        "model",
        r##"{"src": "a.glb", "name": "Gearbox", "format": "glb", "up": "z",
            "color": "#c96", "shading": "wireframe", "yaw": 90, "pitch": -10}"##,
    )
    .markdown();
    assert_eq!(named.as_ref(), "[Gearbox](a.glb)");
}

#[test]
fn model_refuses_a_missing_src_and_unknown_values() {
    assert!(invalid("model", r#"{"src": "  "}"#).contains("src"));
    assert!(invalid("model", r#"{"name": "x"}"#).contains("src"));
    assert!(invalid("model", r#"{"src": "a.obj", "color": "clay"}"#).contains("clay"));
    assert!(invalid("model", r#"{"src": "a.obj", "format": "blend"}"#).contains("blend"));
    assert!(invalid("model", r#"{"src": "a.obj", "shading": "toon"}"#).contains("toon"));
}

#[test]
fn image_takes_one_src_or_several() {
    let one = ready("image", r#"{"src": "shot.png", "alt": "After"}"#).markdown();
    assert_eq!(one.as_ref(), "![After](shot.png)");
    let many = ready(
        "image",
        r#"{"images": [{"src": "a.png"}, {"src": "b.png", "caption": "Dark"}]}"#,
    )
    .markdown();
    assert!(
        many.contains("![](a.png)") && many.contains("![Dark](b.png)"),
        "{many}"
    );
    assert!(invalid("image", "{}").contains("src"));
    assert!(invalid("image", r#"{"src": "  "}"#).contains("src"));
}

#[test]
fn audio_takes_one_src_or_tracks() {
    let one = ready("audio", r#"{"src": "out/take-2.wav"}"#).markdown();
    assert_eq!(one.as_ref(), "- [take-2.wav](out/take-2.wav)");
    let tracks = ready(
        "audio",
        r#"{"tracks": [{"src": "a.mp3", "name": "Intro"}, {"src": "b.mp3"}]}"#,
    )
    .markdown();
    assert!(
        tracks.contains("- [Intro](a.mp3)\n- [b.mp3](b.mp3)"),
        "{tracks}"
    );
    assert!(invalid("audio", "{}").contains("src"));
}

#[test]
fn local_files_resolve_against_the_project_and_home() {
    use super::source::{Located, parse, resolve};
    use std::path::Path;
    let root = Some(Path::new("/work/app"));
    let home = Some(Path::new("/home/me"));
    let local = |src| resolve(parse(src), root, home);
    assert_eq!(
        local("docs/a.png"),
        Located::Path("/work/app/docs/a.png".into())
    );
    assert_eq!(local("/tmp/a.png"), Located::Path("/tmp/a.png".into()));
    assert_eq!(local("~/a.png"), Located::Path("/home/me/a.png".into()));
    assert_eq!(
        local("file:///tmp/a.png"),
        Located::Path("/tmp/a.png".into())
    );
    assert_eq!(
        local("https://x.dev/a.png"),
        Located::Url("https://x.dev/a.png".to_owned())
    );
    assert_eq!(
        resolve(parse("docs/a.png"), None, home),
        Located::Path("docs/a.png".into())
    );
    // No home folder: a home path names nothing rather than a relative one.
    assert_eq!(resolve(parse("~/a.png"), root, None), Located::Invalid);
}

#[test]
fn color_values_are_found_in_prose_and_code() {
    use super::color::color_ranges;
    let found = |text: &str, code: &[std::ops::Range<usize>]| {
        color_ranges(text, code)
            .into_iter()
            .map(|range| text[range].to_owned())
            .collect::<Vec<_>>()
    };
    let prose = "Emerald #10B981, then rgb(255 0 0 / 50%) and oklch(0.7 0.15 250).";
    assert_eq!(
        found(prose, &[]),
        ["#10B981", "rgb(255 0 0 / 50%)", "oklch(0.7 0.15 250)"]
    );
    // Issue numbers stay text; short hex counts in code or with a letter.
    assert!(found("Fixed in #502 and #1234.", &[]).is_empty());
    assert_eq!(found("Muted #ccc text", &[]), ["#ccc"]);
    let code = "Use #333 here";
    let code_span = 4..8;
    assert_eq!(found(code, std::slice::from_ref(&code_span)), ["#333"]);
    // Words that merely contain a function name, entities, and anchors are not colors.
    assert!(found("Call foorgb(1 2 3), see &#123; and page#abcdef0", &[]).is_empty());
    assert!(found("rgb(not a color)", &[]).is_empty());
    assert_eq!(
        found("hsl(210 40% 50%) color(display-p3 1 0 0) hwb(0 0% 0%)", &[]),
        [
            "hsl(210 40% 50%)",
            "color(display-p3 1 0 0)",
            "hwb(0 0% 0%)"
        ]
    );
}

#[test]
fn a_color_copies_as_written_then_in_every_notation() {
    use super::super::actions::CopyKind;
    use super::color::color_copy_entries;
    let entries = color_copy_entries("#FF0000");
    assert_eq!(entries[0].label.as_ref(), "Copy Value");
    assert_eq!(entries[0].text.as_ref(), "#FF0000");
    assert_eq!(entries[0].kind, CopyKind::Primary);
    let labels: Vec<_> = entries[1..]
        .iter()
        .map(|entry| entry.label.as_ref())
        .collect();
    assert_eq!(
        labels,
        [
            "Copy as HEX",
            "Copy as RGB",
            "Copy as HSL",
            "Copy as OKLCH",
            "Copy as OKLab",
            "Copy as Display P3"
        ]
    );
    assert!(
        entries[1..]
            .iter()
            .all(|entry| entry.kind == CopyKind::Alternative)
    );
    assert_eq!(entries[1].text.as_ref(), "#ff0000");
    assert_eq!(entries[2].text.as_ref(), "rgb(255 0 0)");
}

// ---------------------------------------------------------------------------
// Colors

fn close(color: Color, rgb: [f64; 3], alpha: f64) -> bool {
    let near = |a: f64, b: f64| (a - b).abs() < 2e-3;
    near(color.r, rgb[0])
        && near(color.g, rgb[1])
        && near(color.b, rgb[2])
        && near(color.alpha, alpha)
}

#[test]
fn every_notation_reads_the_same_red() {
    for text in [
        "#ff0000",
        "red",
        "rgb(255, 0, 0)",
        "rgb(255 0 0)",
        "rgb(100% 0% 0%)",
        "rgba(255, 0, 0, 1)",
        "hsl(0 100% 50%)",
        "hsl(0deg, 100%, 50%)",
        "hsl(1turn 100% 50%)",
        "hwb(0 0% 0%)",
        "oklab(0.62796 0.22486 0.12585)",
        "oklch(0.62796 0.25768 29.234)",
        "lab(54.29 80.8 69.89)",
        "lch(54.29 106.84 40.85)",
        "color(srgb 1 0 0)",
        "color(display-p3 0.9175 0.2003 0.1386)",
    ] {
        let color = parse(text).unwrap_or_else(|| panic!("{text} should parse"));
        assert!(close(color, [1.0, 0.0, 0.0], 1.0), "{text}: {color:?}");
    }
}

#[test]
fn notations_format_and_parse_back() {
    let accent = parse("#7c5cff").unwrap();
    for notation in Notation::ALL {
        let written = accent.format(notation);
        let back = parse(&written).unwrap_or_else(|| panic!("{written} should parse back"));
        assert!(
            close(back, [accent.r, accent.g, accent.b], accent.alpha),
            "{notation:?}: {written} -> {back:?}"
        );
    }
    assert_eq!(accent.format(Notation::Hex), "#7c5cff");
    assert_eq!(accent.format(Notation::Rgb), "rgb(124 92 255)");
    assert_eq!(
        parse("rgb(0 0 0 / 0.5)").unwrap().format(Notation::Hex),
        "#00000080"
    );
}
