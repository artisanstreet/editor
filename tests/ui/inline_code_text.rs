//! Behavioral coverage for native inline reasoning fragments.
//!
//! Pure parser and summary-line tests need no window: inline code spans,
//! emphasis resolution order, word-edge underscores, literal fallbacks,
//! block-mark stripping, aligned override runs, and headline/sentence
//! summary reduction.

use artisan_ui::inline_code_text::{
    InlineFragment, flatten_fragments, fragment_runs, inline_fragments, summary_line,
};

fn plain(text: &str) -> InlineFragment {
    InlineFragment {
        text: text.to_owned(),
        code: false,
        strong: false,
        em: false,
        strike: false,
    }
}

#[test]
fn code_spans_split_with_faces() {
    let fragments = inline_fragments("read `Cargo.toml` now");
    assert_eq!(fragments.len(), 3);
    assert_eq!(fragments[0], plain("read "));
    assert_eq!(
        fragments[1],
        InlineFragment {
            text: "Cargo.toml".to_owned(),
            code: true,
            ..fragments[1].clone()
        }
    );
    assert!(fragments[1].code);
    assert_eq!(fragments[2], plain(" now"));
}

#[test]
fn unclosed_backtick_reads_rest_as_code() {
    let fragments = inline_fragments("fix `Cargo.toml");
    assert_eq!(fragments.len(), 2);
    assert_eq!(fragments[0], plain("fix "));
    assert!(fragments[1].code);
    assert_eq!(fragments[1].text, "Cargo.toml");
}

#[test]
fn strong_and_emphasis_resolve_longest_first() {
    let fragments = inline_fragments("**bold** and *italic*");
    assert!(
        fragments
            .iter()
            .any(|fragment| fragment.strong && fragment.text == "bold")
    );
    assert!(
        fragments
            .iter()
            .any(|fragment| fragment.em && fragment.text == "italic")
    );
    assert!(
        fragments
            .iter()
            .all(
                |fragment| !fragment.text.contains("**") && !fragment.text.contains('*')
                    || fragment.code
            )
    );
}

#[test]
fn strikethrough_and_underscore_edges() {
    let fragments = inline_fragments("~~gone~~ and inspection_types stay");
    assert!(
        fragments
            .iter()
            .any(|fragment| fragment.strike && fragment.text == "gone")
    );
    assert!(
        fragments
            .iter()
            .any(|fragment| fragment.text.contains("inspection_types"))
    );
    assert!(
        fragments
            .iter()
            .all(|fragment| !fragment.text.contains("~~"))
    );
}

#[test]
fn unmatched_marks_stay_literal() {
    let fragments = inline_fragments("2 * 3 = 6 *");
    assert_eq!(flatten_fragments(&fragments), "2 * 3 = 6 *");
}

#[test]
fn block_marks_strip_per_line() {
    assert_eq!(flatten_fragments(&inline_fragments("## Head")), "Head");
    assert_eq!(flatten_fragments(&inline_fragments("- item")), "item");
    assert_eq!(flatten_fragments(&inline_fragments("> quoted")), "quoted");
    assert_eq!(flatten_fragments(&inline_fragments("3. third")), "third");
}

#[test]
fn code_ranges_get_aligned_runs_for_family_overrides() {
    let (flat, highlights, code_ranges) = fragment_runs(&inline_fragments("read `Cargo.toml` now"));
    assert_eq!(flat, "read Cargo.toml now");
    // The override only applies to an existing run fully inside it, so
    // every code range must own an aligned highlight range even when the
    // run is otherwise visually default.
    assert_eq!(code_ranges, vec![5..15]);
    for range in &code_ranges {
        assert!(
            highlights.iter().any(|(highlight, _)| highlight == range),
            "every code range needs an aligned run"
        );
    }
}

#[test]
fn latest_headline_wins_the_summary_line() {
    let text = "**First thought**\n\nSome body.\n\n**Planning playful ambiguous response**";
    assert_eq!(
        summary_line(text),
        Some("Planning playful ambiguous response".to_owned())
    );
}

#[test]
fn first_finished_sentence_stands_in_without_headline() {
    assert_eq!(
        summary_line("Considering options. Then more detail here."),
        Some("Considering options.".to_owned())
    );
}

#[test]
fn unfinished_phases_yield_no_line() {
    assert_eq!(summary_line("Considering options without an ending"), None);
    assert_eq!(summary_line(""), None);
    assert_eq!(summary_line("   \n\n  "), None);
}

#[test]
fn whitespace_collapses_to_one_row() {
    assert_eq!(
        summary_line("Thinking   hard\nabout  this."),
        Some("Thinking hard about this.".to_owned())
    );
}

#[test]
fn multibyte_prose_never_splits_boundaries() {
    assert_eq!(
        flatten_fragments(&inline_fragments("Whoopty 🎉")),
        "Whoopty 🎉"
    );
    assert_eq!(
        flatten_fragments(&inline_fragments("café au lait")),
        "café au lait"
    );
    assert_eq!(
        flatten_fragments(&inline_fragments("日本語テスト")),
        "日本語テスト"
    );
    let marked = inline_fragments("`🎉` party *日本*");
    assert!(
        marked
            .iter()
            .any(|fragment| fragment.code && fragment.text == "🎉")
    );
    assert!(
        marked
            .iter()
            .any(|fragment| fragment.em && fragment.text == "日本")
    );
    assert_eq!(flatten_fragments(&marked), "🎉 party 日本");
}

#[test]
fn nested_marks_resolve_inside_out() {
    let fragments = inline_fragments("**bold with *italic* inside**");
    assert!(
        fragments
            .iter()
            .any(|fragment| fragment.strong && fragment.em && fragment.text == "italic")
    );
    assert!(
        fragments
            .iter()
            .any(|fragment| fragment.strong && !fragment.em && fragment.text == "bold with ")
    );
    assert!(
        fragments
            .iter()
            .all(|fragment| !fragment.text.contains("**") && !fragment.text.contains('*'))
    );
}
