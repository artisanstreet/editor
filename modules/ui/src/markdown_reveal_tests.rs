use gpui::{FontWeight, HighlightStyle};

use super::*;
use crate::markdown::{Block, MarkdownEngine, Span};

#[test]
fn trailing_partial_word_is_held() {
    assert_eq!(stable_reveal_end("Hello wor"), "Hello ".len());
    assert_eq!(stable_reveal_end("Hello world "), "Hello world ".len());
    assert_eq!(stable_reveal_end("Hello"), 0);
}

#[test]
fn unclosed_inline_constructs_are_held_until_closed() {
    assert_eq!(stable_reveal_end("Some **bold te"), "Some ".len());
    assert_eq!(stable_reveal_end("Use `cargo b"), "Use ".len());
    assert_eq!(stable_reveal_end("See [docs](https://exa"), "See ".len());
    assert_eq!(
        stable_reveal_end("Some **bold** text "),
        "Some **bold** text ".len()
    );
}

#[test]
fn intraword_underscores_and_escapes_never_hold() {
    assert_eq!(
        stable_reveal_end("call foo_bar now "),
        "call foo_bar now ".len()
    );
    assert_eq!(stable_reveal_end("2 \\* 3 is "), "2 \\* 3 is ".len());
}

#[test]
fn a_closed_paragraph_releases_its_literal_openers() {
    let source = "Costs 2 * 3 units.\n\nNext ";
    assert_eq!(stable_reveal_end(source), source.len());
}

#[test]
fn open_fences_are_held_from_their_line() {
    let source = "Intro\n\n```rust\nfn main() {";
    assert_eq!(stable_reveal_end(source), "Intro\n\n".len());
    let unterminated_close = "Intro\n\n```\nx\n```";
    assert_eq!(stable_reveal_end(unterminated_close), "Intro\n\n".len());
    let closed = "Intro\n\n```\nx\n```\n";
    assert_eq!(stable_reveal_end(closed), closed.len());
}

#[test]
fn tables_are_held_until_terminated() {
    let header_only = "Intro\n\n| a | b |\n";
    assert_eq!(stable_reveal_end(header_only), "Intro\n\n".len());
    let open = "Intro\n\n| a | b |\n|---|---|\n| 1 | 2 |\n";
    assert_eq!(stable_reveal_end(open), "Intro\n\n".len());
    let terminated = "Intro\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n";
    assert_eq!(stable_reveal_end(terminated), terminated.len());
}

#[test]
fn marker_only_lines_are_held() {
    assert_eq!(stable_reveal_end("Para\n\n- "), "Para\n\n".len());
    assert_eq!(stable_reveal_end("Para\n\n## "), "Para\n\n".len());
    assert_eq!(stable_reveal_end("Para\n\n12. "), "Para\n\n".len());
    assert_eq!(stable_reveal_end("Para\n\n- one "), "Para\n\n- one ".len());
}

#[test]
fn steps_reveal_whole_words_and_whole_constructs() {
    let source = "Hello **big bold** world ";
    let steps = RevealSteps::new(source);
    assert_eq!(steps.next(0), "Hello ".len());
    assert_eq!(steps.next("Hello ".len()), "Hello **big bold** ".len());
    assert_eq!(steps.next("Hello **big bold** ".len()), source.len());
    assert_eq!(steps.next(source.len()), source.len());
}

#[test]
fn steps_reveal_fences_and_tables_as_one_unit() {
    let source = "Intro\n\n```\na b c\n```\n\n| a |\n|---|\n| 1 |\n\nAfter ";
    let steps = RevealSteps::new(source);
    let fence = "Intro\n\n".len();
    assert_eq!(steps.next(0), fence);
    let table = source.find("| a |").expect("table");
    assert_eq!(steps.next(fence), table);
    assert_eq!(steps.next(table), source.find("After").expect("after"));
}

#[test]
fn steps_carry_a_list_marker_with_its_first_word() {
    let source = "Para\n\n- item one ";
    let steps = RevealSteps::new(source);
    let list = "Para\n\n".len();
    assert_eq!(steps.next(0), list);
    assert_eq!(steps.next(list), "Para\n\n- item ".len());
}

#[test]
fn fade_spans_cover_only_translucent_segments() {
    let fade = RevealFade::new(vec![(5, 1.0), (10, 0.5), (14, 0.0)]);
    assert!(!fade.is_opaque());
    assert!((fade.alpha_at(3) - 1.0).abs() < f32::EPSILON);
    assert!((fade.alpha_at(12) - 0.5).abs() < f32::EPSILON);
    assert_eq!(fade.leaf_spans(8, 10), vec![(2..6, 0.5), (6..10, 0.0)]);
    assert!(fade.leaf_spans(0, 5).is_empty());
}

#[test]
fn fade_overlays_keep_base_styles_disjoint() {
    let bold = HighlightStyle {
        font_weight: Some(FontWeight::BOLD),
        ..HighlightStyle::default()
    };
    let merged = apply_reveal_fade(&[(0..6, bold)], &[(4..10, 0.25)]);
    assert_eq!(merged.len(), 3);
    assert_eq!(merged[0], (0..4, bold));
    assert_eq!(merged[1].0, 4..6);
    assert_eq!(merged[1].1.font_weight, Some(FontWeight::BOLD));
    assert_eq!(merged[1].1.fade_out, Some(0.75));
    assert_eq!(merged[2].0, 6..10);
    assert_eq!(merged[2].1.font_weight, None);
}

/// One painted character with everything that decides its glyph metrics
/// and line: the block path it sits in and its inline styling.
#[derive(Clone, Debug, PartialEq)]
struct StyledChar {
    path: String,
    style: String,
    character: char,
}

fn flatten_spans(spans: &[Span], path: &str, style: &str, out: &mut Vec<StyledChar>) {
    for span in spans {
        match span {
            Span::Text(text) | Span::Html(text) => push_chars(text, path, style, out),
            Span::Code(text) => push_chars(text, path, &format!("{style}c"), out),
            Span::Emphasis(inner) => flatten_spans(inner, path, &format!("{style}e"), out),
            Span::Strong(inner) => flatten_spans(inner, path, &format!("{style}s"), out),
            Span::Link { label, .. } => flatten_spans(label, path, &format!("{style}l"), out),
        }
    }
}

fn push_chars(text: &str, path: &str, style: &str, out: &mut Vec<StyledChar>) {
    out.extend(text.chars().map(|character| StyledChar {
        path: path.to_owned(),
        style: style.to_owned(),
        character,
    }));
}

fn flatten_blocks(blocks: &[Block], path: &str, out: &mut Vec<StyledChar>) {
    for (index, block) in blocks.iter().enumerate() {
        match block {
            Block::Heading { level, spans, .. } => {
                flatten_spans(spans, &format!("{path}/{index}h{level}"), "", out);
            }
            Block::Paragraph { spans, .. } => {
                flatten_spans(spans, &format!("{path}/{index}p"), "", out);
            }
            Block::List { ordered, items, .. } => {
                for (item_index, item) in items.iter().enumerate() {
                    let item_path = format!("{path}/{index}l{ordered}/{item_index}");
                    flatten_blocks(&item.blocks, &item_path, out);
                }
            }
            // Atomic blocks paint whole: one token holding all their content.
            other => push_chars(
                &format!("\u{1}{:?}\u{2}", other.text_content()),
                &format!("{path}/{index}atomic"),
                "",
                out,
            ),
        }
    }
}

fn painted(engine: &MarkdownEngine, source: &str) -> Vec<StyledChar> {
    let document = engine.parse_document(source).expect("parse");
    let mut out = Vec::new();
    flatten_blocks(document.blocks(), "", &mut out);
    out
}

/// The guarantee: for every streamed prefix, what the stable cutoff paints
/// is a prefix of what the finished reply paints, so nothing on screen ever
/// changes line, style, or block.
fn assert_never_shifts(final_source: &str) {
    let engine = MarkdownEngine::new().expect("engine");
    let finished = painted(&engine, final_source);
    for (cut, _) in final_source.char_indices().skip(1) {
        let streamed = &final_source[..cut];
        let stable = &streamed[..stable_reveal_end(streamed)];
        let shown = painted(&engine, stable);
        assert!(
            finished.starts_with(&shown),
            "prefix {streamed:?} reveals {stable:?}, which paints differently from the finished reply"
        );
        let steps = RevealSteps::new(stable);
        let mut revealed = 0;
        while revealed < stable.len() {
            let next = steps.next(revealed);
            assert!(next > revealed, "stepping {stable:?} stalled at {revealed}");
            revealed = next;
            let step = painted(&engine, &stable[..revealed]);
            assert!(
                finished.starts_with(&step),
                "stepping {stable:?} shows {:?}, which paints differently from the finished reply",
                &stable[..revealed]
            );
        }
    }
}

#[test]
fn streamed_prose_never_shifts() {
    assert_never_shifts(
        "The quick brown fox jumps over **the lazy dog** and keeps `running` far away.\n\n\
         A second paragraph links [the docs](https://example.com/docs) and ends here.\n",
    );
}

#[test]
fn streamed_lists_and_headings_never_shift() {
    assert_never_shifts(
        "## Summary\n\nWhat changed:\n\n- First item with *emphasis*\n- Second item\n  continues here\n\n\
         1. Ordered one\n2. Ordered two\n\nDone.\n",
    );
}

#[test]
fn streamed_fences_and_tables_never_shift() {
    assert_never_shifts(
        "Run this:\n\n```sh\ncargo test | tee log\n```\n\nResults:\n\n| Suite | Passed |\n|---|---|\n\
         | ui | 12 |\n| core | 40 |\n\nAll green.\n",
    );
}
