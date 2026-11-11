use aqua_text::{FontFamily, FontSlant, FontWeight, TextStyle};
use editor_ui::inline_code_text::inline_runs;
use editor_ui::theme::{ArtisanTheme, ProseTypography};

#[test]
fn commonmark_nested_marks_and_code_use_flattened_utf8_ranges() {
    let runs =
        inline_runs(r"é ``a ` b`` and **bold _nested_** \*literal\* [label](https://example.com)");
    assert_eq!(runs.text, "é a ` b and bold nested *literal* label");
    let code = runs.ranges.iter().find(|run| run.style.code).unwrap();
    assert_eq!(&runs.text[code.range.clone()], "a ` b");
    assert_eq!(code.range.start, 3);
    let nested = runs
        .ranges
        .iter()
        .find(|run| run.style.strong && run.style.em)
        .unwrap();
    assert_eq!(&runs.text[nested.range.clone()], "nested");
}

#[test]
fn block_breaks_and_code_precedence_preserve_source_rules() {
    let runs = inline_runs("  ~~crossed~~\nnext\n\n**`mono`**\n\n```\nblock\n```\n");
    assert_eq!(runs.text, "crossed next mono block");
    assert!(
        runs.ranges
            .iter()
            .any(|run| run.style.strike && &runs.text[run.range.clone()] == "crossed")
    );
    let code = runs
        .ranges
        .iter()
        .find(|run| run.style.code && run.style.strong)
        .unwrap();
    assert_eq!(&runs.text[code.range.clone()], "mono");
    assert!(
        runs.ranges
            .iter()
            .any(|run| run.style.code && runs.text[run.range.clone()].contains("block"))
    );
}

#[test]
fn painted_runs_apply_source_overrides_and_preserve_raw_color() {
    let theme = ArtisanTheme::resolved();
    let color = theme.colors.text.to_reference_text_color();
    let base = TextStyle::default();
    let runs = inline_runs("**bold _italic_ `mono`** ~~struck~~");
    let spans = runs.styled_spans(&base, color, &theme);
    let mono = spans.iter().find(|span| span.text == "mono").unwrap();
    assert_eq!(mono.style.weight, FontWeight::NORMAL);
    assert_eq!(
        mono.style.families,
        vec![FontFamily::Named(theme.typography.mono.family.to_owned())]
    );
    assert_eq!(
        mono.source_color_hsla,
        Some([
            color.hue_degrees,
            color.saturation,
            color.lightness,
            color.alpha
        ])
    );
    let italic = spans.iter().find(|span| span.text == "italic").unwrap();
    assert_eq!(italic.style.weight, ProseTypography::STRONG_WEIGHT);
    assert_eq!(italic.style.slant, FontSlant::Italic);
    assert!(
        spans
            .iter()
            .any(|span| span.text == "struck" && span.style.strikethrough)
    );
}
