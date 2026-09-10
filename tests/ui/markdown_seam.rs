//! External coverage for the Markdown engine seam in `artisan_ui`.
//!
//! These tests exercise the public `artisan_ui::markdown` API and the pure
//! `artisan_ui::markdown_renderer::present_inline` presentation helper, and
//! pin the behaviors this lane requires: raw HTML inertness, open-fence
//! fallback, closed-fence highlighting over byte ranges, deterministic owned
//! output, plus list parity (tight, loose, ordered, nested with exact trees,
//! task), inline emphasis/strong/link preservation, single-call merged
//! highlight runs, and dropless truncated prefixes.

use artisan_ui::markdown::{Block, CodeFence, CodeTokenKind, MarkdownEngine, Span};
use artisan_ui::markdown_renderer::present_inline;
use artisan_ui::theme::{ArtisanTheme, ThemeMode};
use gpui::{FontStyle, FontWeight};

const CLOSED_RUST_FENCE: &str =
    "// leading\nfn main() {\n    let message = \"artisan\";\n    let count = 42;\n}\n";

/// Builds a fresh engine; construction is fallible by contract.
fn engine() -> MarkdownEngine {
    MarkdownEngine::new().expect("markdown engine construction must succeed")
}

/// Builds the dark theme the presentation helper styles against.
fn theme() -> ArtisanTheme {
    ArtisanTheme::for_mode(ThemeMode::Dark)
}

/// Slices `source` with a stored token range, proving the offsets address the
/// original fence body rather than a re-derivation.
fn slice<'a>(source: &'a str, range: &std::ops::Range<usize>) -> &'a str {
    &source[range.clone()]
}

/// Collects every code fence from a block list.
fn fences(blocks: &[Block]) -> Vec<&CodeFence> {
    blocks
        .iter()
        .filter_map(|block| match block {
            Block::Code(fence) => Some(fence),
            _ => None,
        })
        .collect()
}

#[test]
fn parses_headings_paragraphs_and_inline_code() {
    let parsed = engine()
        .parse_document("# Alpha\n\nbefore `x + y` after\n")
        .expect("parsing must succeed");
    let blocks = parsed.blocks();

    let Some(Block::Heading {
        level: 1, spans, ..
    }) = blocks.first()
    else {
        panic!("expected a leading heading, got {blocks:?}");
    };
    assert_eq!(spans.len(), 1);
    assert_eq!(spans.first(), Some(&Span::Text("Alpha".to_owned())));

    let Some(Block::Paragraph {
        spans: paragraph, ..
    }) = blocks.get(1)
    else {
        panic!("expected one paragraph after the heading, got {blocks:?}");
    };
    assert_eq!(
        paragraph,
        &vec![
            Span::Text("before ".to_owned()),
            Span::Code("x + y".to_owned()),
            Span::Text(" after".to_owned()),
        ]
    );
}

#[test]
fn carries_raw_html_as_inert_data() {
    let script = "<script>alert(\"owned\")</script>";
    let inline = "<em>kept</em>";
    let input = format!("{script}\n\nplain {inline} tail\n");

    let parsed = engine().parse_document(&input).expect("parse succeeds");

    let html_blocks = parsed
        .blocks()
        .iter()
        .filter_map(|block| match block {
            Block::Html { source } => Some(source.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    // Verbatim means the terminating newline of the HTML block line stays.
    let expected_block = format!("{script}\n");
    assert_eq!(
        html_blocks,
        vec![expected_block.as_str()],
        "block HTML must round-trip verbatim"
    );

    // `pulldown-cmark` delivers inline HTML as separate open/close chunks
    // around the inner text run; each chunk stays verbatim and inert.
    let expected_paragraph = &[
        Span::Text("plain ".to_owned()),
        Span::Html("<em>".to_owned()),
        Span::Text("kept".to_owned()),
        Span::Html("</em>".to_owned()),
        Span::Text(" tail".to_owned()),
    ];
    let paragraph = parsed
        .blocks()
        .iter()
        .find_map(|block| match block {
            Block::Paragraph { spans, .. } => Some(spans),
            _ => None,
        })
        .expect("inline fixture keeps its paragraph");
    assert_eq!(paragraph.as_slice(), expected_paragraph);

    // Nothing outside the inert payloads may have interpreted the markup:
    // ordinary text runs must read exactly as authored, with no derived
    // emphasis, entities, or element structure.
    let rendered_text = paragraph
        .iter()
        .filter_map(|span| match span {
            Span::Text(text) | Span::Code(text) => Some(text.as_str()),
            Span::Html(_) => None,
        })
        .collect::<Vec<_>>()
        .join("");
    assert!(!rendered_text.contains("alert"));
    assert!(!rendered_text.contains('<'));
    assert_eq!(rendered_text, "plain kept tail");
}

#[test]
fn open_fence_stays_plain_until_it_closes() {
    let body = "fn unfinished() {\n";
    let input = format!("```rust\n{body}");

    let parsed = engine().parse_document(&input).expect("parse succeeds");

    let collected = fences(parsed.blocks());
    assert_eq!(collected.len(), 1);
    let fence = collected[0];
    assert_eq!(fence.language.as_deref(), Some("rust"));
    assert!(!fence.closed, "a missing closing line must read as open");
    assert_eq!(fence.source, body);
    assert!(fence.tokens.is_none(), "an open fence never highlights");
}

#[test]
fn shorter_marker_run_does_not_close_a_longer_fence() {
    let body = "fn unfinished() {}\n```\n";
    let input = format!("````rust\n{body}");

    let parsed = engine().parse_document(&input).expect("parse succeeds");

    let collected = fences(parsed.blocks());
    assert_eq!(collected.len(), 1);
    let fence = collected[0];
    assert_eq!(fence.source, body);
    assert!(
        !fence.closed,
        "three markers cannot close a four-marker fence"
    );
    assert!(fence.tokens.is_none(), "the still-open fence stays plain");
}

#[test]
fn closed_rust_fence_is_highlighted_over_byte_ranges() {
    let input = format!("```rust\n{CLOSED_RUST_FENCE}```\n");

    let parsed = engine().parse_document(&input).expect("parse succeeds");

    let collected = fences(parsed.blocks());
    assert_eq!(collected.len(), 1);
    let fence = collected[0];
    assert_eq!(fence.language.as_deref(), Some("rust"));
    assert!(fence.closed);
    assert_eq!(fence.source, CLOSED_RUST_FENCE);

    let tokens = fence
        .tokens
        .as_ref()
        .expect("closed known fence highlights");
    assert!(!tokens.is_empty(), "no classified ranges were produced");

    let mut previous_end = 0;
    for token in tokens {
        assert!(
            token.range.start >= previous_end,
            "ranges must stay ordered and non-overlapping"
        );
        assert!(
            token.range.end <= fence.source.len(),
            "range {:?} escapes the fence body",
            token.range
        );
        previous_end = token.range.end;
    }

    let body: &str = fence.source.as_str();
    let classified = |kind: CodeTokenKind| -> Vec<&str> {
        tokens
            .iter()
            .filter(|token| token.kind == kind)
            .map(|token| slice(body, &token.range))
            .collect()
    };

    assert!(
        classified(CodeTokenKind::Comment)
            .iter()
            .any(|text| text.contains("// leading")),
        "comments must classify, got {:?}",
        classified(CodeTokenKind::Comment)
    );
    // The bundled Rust grammar scopes `fn` as `storage.type.function`, which
    // this seam maps to [`CodeTokenKind::Type`].
    assert!(
        classified(CodeTokenKind::Type)
            .iter()
            .any(|text| text == &"fn"),
        "`fn` must classify as a type-like keyword, got {:?}",
        classified(CodeTokenKind::Type)
    );
    assert!(
        classified(CodeTokenKind::Function)
            .iter()
            .any(|text| text == &"main"),
        "function names must classify, got {:?}",
        classified(CodeTokenKind::Function)
    );
    assert!(
        classified(CodeTokenKind::Str)
            .iter()
            .any(|text| text.contains("artisan")),
        "strings must cover the literal, got {:?}",
        classified(CodeTokenKind::Str)
    );
    assert!(
        classified(CodeTokenKind::Number)
            .iter()
            .any(|text| text == &"42"),
        "numbers must cover 42, got {:?}",
        classified(CodeTokenKind::Number)
    );
}

#[test]
fn unknown_language_falls_back_to_unhighlighted_source() {
    let body = "SELECT nothing FROM nowhere;\n";
    let input = format!("```definitely-not-a-language\n{body}```\n");

    let parsed = engine().parse_document(&input).expect("parse succeeds");

    let collected = fences(parsed.blocks());
    assert_eq!(collected.len(), 1);
    let fence = collected[0];
    assert_eq!(fence.language.as_deref(), Some("definitely-not-a-language"));
    assert!(fence.closed);
    assert_eq!(fence.source, body);
    assert!(fence.tokens.is_none());
}

#[test]
fn repeated_parses_are_identical_owned_documents() {
    let input = "# Heading\n\nintro `code`\n\n```rust\nlet done = true;\n```\n";

    let first = engine().parse_document(input).expect("first parse");
    let second = engine().parse_document(input).expect("second parse");

    assert_eq!(first, second, "the seam must stay deterministic");
}

/// Exact assistant longform from `parity_visual_proof::LONGFORM_ASSISTANT_BODY`.
/// Regression for the wide-capture fault that dropped the three-item
/// checklist after "Remaining checks".
const LONGFORM_ASSISTANT_BODY: &str = "## Result\n\nThe shell, transcript, and composer match the reference.\n\n```rust\nlet content_width = window_width - sidebar_width;\n```\n\nRemaining checks:\n\n- narrow viewport keeps the inspector\n- wide viewport keeps the composer docked\n- [runbook](https://example.invalid/runbook) attached\n";

/// Exact user longform from `parity_visual_proof::LONGFORM_USER_BODY`.
const LONGFORM_USER_BODY: &str = "# Parity drill\n\nProve the transcript keeps structure:\n\n- heading survives\n- `code span` survives\n- [reference link](https://example.invalid/parity) survives\n\n```text\nplain fenced block\n```\n";

/// Flattens spans to visible text for no-drop assertions.
fn visible(spans: &[Span]) -> String {
    spans.iter().map(Span::text_content).collect()
}

fn paragraph_texts(blocks: &[Block]) -> Vec<String> {
    blocks
        .iter()
        .filter_map(|block| match block {
            Block::Paragraph { spans, .. } => Some(visible(spans)),
            _ => None,
        })
        .collect()
}

fn lists<'a>(
    blocks: &'a [Block],
) -> Vec<(&'a bool, &'a Option<u64>, &'a Vec<artisan_ui::markdown::ListItem>)> {
    blocks
        .iter()
        .filter_map(|block| match block {
            Block::List {
                ordered, start, items, ..
            } => Some((ordered, start, items)),
            _ => None,
        })
        .collect()
}

#[test]
fn longform_assistant_checklist_survives_with_link_destination() {
    let parsed = engine()
        .parse_document(LONGFORM_ASSISTANT_BODY)
        .expect("parse succeeds");
    let blocks = parsed.blocks();

    let collected = lists(blocks);
    assert_eq!(collected.len(), 1, "one checklist, got {blocks:?}");
    let (ordered, _, items) = collected[0];
    assert!(!ordered, "assistant checklist is unordered");
    assert_eq!(items.len(), 3, "all three checks survive");

    assert_eq!(
        items[0].text_content(),
        "narrow viewport keeps the inspector"
    );
    assert_eq!(
        items[1].text_content(),
        "wide viewport keeps the composer docked"
    );
    assert!(
        items[2].text_content().contains("runbook")
            && items[2].text_content().contains("attached"),
        "third item keeps its label, got {:?}",
        items[2].text_content()
    );

    let third = items[2]
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Paragraph { spans, .. } => Some(spans),
            _ => None,
        })
        .expect("tight item settles as a paragraph");
    let link = third.iter().find_map(|span| match span {
        Span::Link { label, destination } => Some((label, destination)),
        _ => None,
    });
    let (label, destination) = link.expect("runbook link survives with its destination");
    assert_eq!(destination, "https://example.invalid/runbook");
    assert_eq!(visible(label), "runbook");

    // Nothing before the list was disturbed: heading, prose, and fence stay.
    assert!(matches!(blocks.first(), Some(Block::Heading { level: 2, .. })));
    assert_eq!(fences(blocks).len(), 1);
    assert!(
        paragraph_texts(blocks)
            .iter()
            .any(|text| text.contains("Remaining checks")),
        "the checks lead-in survives, got {blocks:?}"
    );
}

#[test]
fn longform_user_list_preserves_code_and_link() {
    let parsed = engine()
        .parse_document(LONGFORM_USER_BODY)
        .expect("parse succeeds");
    let blocks = parsed.blocks();

    let collected = lists(blocks);
    assert_eq!(collected.len(), 1);
    assert_eq!(collected[0].2.len(), 3);

    let items = collected[0].2;
    assert_eq!(items[0].text_content(), "heading survives");
    assert_eq!(items[1].text_content(), "code span survives");
    assert!(items[2].text_content().contains("reference link"));

    let second = items[1]
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Paragraph { spans, .. } => Some(spans),
            _ => None,
        })
        .expect("second item has a paragraph");
    assert!(
        second
            .iter()
            .any(|span| matches!(span, Span::Code(code) if code == "code span")),
        "inline code survives inside the list, got {second:?}"
    );
}

#[test]
fn ordered_loose_nested_and_task_lists_preserve_every_row() {
    let parsed = engine()
        .parse_document(
            "1. one\n2. two\n\n- loose alpha\n\n- loose beta\n\n- outer\n  - inner\n\n- [ ] open\n- [x] done\n",
        )
        .expect("parse succeeds");
    let blocks = parsed.blocks();
    let collected = lists(blocks);
    assert_eq!(collected.len(), 4, "four lists, got {blocks:?}");

    let (ordered, start, items) = collected[0];
    assert!(ordered);
    assert_eq!(*start, Some(1));
    assert_eq!(
        items
            .iter()
            .map(|item| item.text_content())
            .collect::<Vec<_>>(),
        vec!["one".to_owned(), "two".to_owned()]
    );

    let loose = &collected[1].2;
    assert_eq!(loose.len(), 2);
    for item in loose.iter() {
        assert!(
            item.blocks.iter().any(|block| matches!(
                block,
                Block::Paragraph { .. }
            )),
            "loose items keep paragraph blocks, got {item:?}"
        );
    }
    assert_eq!(loose[0].text_content(), "loose alpha");

    let outer = &collected[2].2;
    assert_eq!(outer.len(), 1);
    let nested = outer[0]
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::List { items, .. } => Some(items),
            _ => None,
        })
        .expect("nested list survives inside its parent item");
    assert_eq!(nested.len(), 1);
    assert_eq!(nested[0].text_content(), "inner");
    assert!(
        outer[0].text_content().contains("outer") && outer[0].text_content().contains("inner"),
        "outer text is not lost beside its nested list"
    );

    let tasks = &collected[3].2;
    assert_eq!(tasks.len(), 2);
    assert_eq!(tasks[0].task, Some(false));
    assert_eq!(tasks[1].task, Some(true));
    assert_eq!(tasks[0].text_content(), "open");
    assert_eq!(tasks[1].text_content(), "done");
}

/// Task markers arrive because the engine enables `ENABLE_TASKLISTS`
/// explicitly: without that option `pulldown-cmark` never emits
/// `TaskListMarker` and these items would read as plain text.
#[test]
fn task_markers_need_no_renderer_to_classify() {
    let parsed = engine()
        .parse_document("- [ ] open\n- [x] done\n")
        .expect("parse succeeds");
    let found = lists(parsed.blocks());
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].2.len(), 2);
    assert_eq!(found[0].2[0].task, Some(false));
    assert_eq!(found[0].2[1].task, Some(true));
}

#[test]
fn nested_list_keeps_exact_tree() {
    let parsed = engine()
        .parse_document("- outer one\n  - inner one\n  - inner two\n- outer two\n")
        .expect("parse succeeds");
    let blocks = parsed.blocks();
    let found = lists(blocks);
    assert_eq!(found.len(), 1, "one top-level list, got {blocks:?}");
    let (ordered, _, outer) = found[0];
    assert!(!ordered);
    assert_eq!(outer.len(), 2, "two outer items, got {outer:?}");

    // The first outer item owns exactly its paragraph plus the nested list:
    // the parent text must not leak into the child list, and the child list
    // must not strand at the document root.
    assert_eq!(outer[0].blocks.len(), 2, "outer text + nested list");
    let first_paragraph = match &outer[0].blocks[0] {
        Block::Paragraph { spans, .. } => visible(spans),
        other => panic!("first outer block is its own paragraph, got {other:?}"),
    };
    assert_eq!(first_paragraph, "outer one");
    let nested = match &outer[0].blocks[1] {
        Block::List { ordered, items, .. } => {
            assert!(!ordered, "nested list is unordered");
            items
        }
        other => panic!("second outer block is the nested list, got {other:?}"),
    };
    assert_eq!(nested.len(), 2, "two inner items, got {nested:?}");
    assert_eq!(nested[0].text_content(), "inner one");
    assert_eq!(nested[1].text_content(), "inner two");
    assert_eq!(nested[0].blocks.len(), 1);
    assert!(matches!(nested[0].blocks[0], Block::Paragraph { .. }));

    assert_eq!(outer[1].blocks.len(), 1);
    assert_eq!(outer[1].text_content(), "outer two");
}

#[test]
fn emphasis_strong_and_links_survive_with_destinations() {
    let parsed = engine()
        .parse_document("A *soft* word, a **hard** word, and a [label](https://example.invalid/x).\n")
        .expect("parse succeeds");
    let paragraph = parsed
        .blocks()
        .iter()
        .find_map(|block| match block {
            Block::Paragraph { spans, .. } => Some(spans),
            _ => None,
        })
        .expect("paragraph survives");
    assert!(
        paragraph.iter().any(|span| matches!(
            span,
            Span::Emphasis(inner) if visible(inner) == "soft"
        )),
        "emphasis survives, got {paragraph:?}"
    );
    assert!(
        paragraph.iter().any(|span| matches!(
            span,
            Span::Strong(inner) if visible(inner) == "hard"
        )),
        "strong survives, got {paragraph:?}"
    );
    let (label, destination) = paragraph
        .iter()
        .find_map(|span| match span {
            Span::Link { label, destination } => Some((label, destination)),
            _ => None,
        })
        .expect("link survives");
    assert_eq!(visible(label), "label");
    assert_eq!(destination, "https://example.invalid/x");
}

#[test]
fn unsafe_link_destination_stays_inert_but_keeps_its_label() {
    let parsed = engine()
        .parse_document("Catch [me](javascript:alert(1)) here.\n")
        .expect("parse succeeds");
    let paragraph = parsed
        .blocks()
        .iter()
        .find_map(|block| match block {
            Block::Paragraph { spans, .. } => Some(spans),
            _ => None,
        })
        .expect("paragraph survives");
    // The engine keeps the authored destination verbatim; the renderer (not
    // this seam) falls back to the plain label for unsafe schemes.
    assert!(
        paragraph.iter().any(|span| matches!(
            span,
            Span::Link { label, destination }
            if visible(label) == "me" && destination.starts_with("javascript:")
        )),
        "unsafe destination stays inert data with its label, got {paragraph:?}"
    );
    assert!(
        visible(paragraph).contains("Catch me here"),
        "no visible copy is dropped, got {:?}",
        visible(paragraph)
    );
}

#[test]
fn truncated_streaming_prefix_drops_no_confirmed_text() {
    // `pulldown-cmark` emits balanced `Start`/`End` pairs even for truncated
    // input (unclosed fences still close as open, unmatched delimiters stay
    // literal text), so every confirmed label below arrives through ordinary
    // events: no end-of-input recovery layer exists to test.
    let input = "Remaining checks:\n\n- narrow viewport keeps the insp\n- **bold tail";
    let parsed = engine().parse_document(input).expect("parse succeeds");
    let joined = parsed
        .blocks()
        .iter()
        .map(Block::text_content)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(joined.contains("Remaining checks"), "lead-in survives");
    assert!(
        joined.contains("narrow viewport keeps the insp"),
        "truncated item survives, got {joined:?}"
    );
    assert!(joined.contains("bold tail"), "unclosed strong label survives");
    assert_eq!(
        joined.matches("narrow viewport keeps the insp").count(),
        1,
        "no duplicated tail, got {joined:?}"
    );

    let link_tail = engine()
        .parse_document("See [runbook](https://example.invalid/runb")
        .expect("parse succeeds");
    let link_joined = link_tail
        .blocks()
        .iter()
        .map(Block::text_content)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        link_joined.contains("runbook"),
        "unclosed link label survives, got {link_joined:?}"
    );

    // An unterminated fence still closes through the balanced event stream
    // and settles open and plain rather than vanishing.
    let fence_tail = engine()
        .parse_document("```rust\nlet half = 1;\n")
        .expect("parse succeeds");
    let collected = fences(fence_tail.blocks());
    assert_eq!(collected.len(), 1);
    assert!(!collected[0].closed);
    assert!(collected[0].tokens.is_none());
    assert!(collected[0].source.contains("let half"));
}

/// Parses one paragraph and presents its spans through the single merged
/// highlight path the renderer feeds to `with_highlights`.
fn presented(source: &str) -> artisan_ui::markdown_renderer::InlinePresentation {
    let parsed = engine().parse_document(source).expect("parse succeeds");
    let spans = parsed
        .blocks()
        .iter()
        .find_map(|block| match block {
            Block::Paragraph { spans, .. } => Some(spans.clone()),
            _ => None,
        })
        .expect("fixture keeps its paragraph");
    present_inline(&spans, theme())
}

/// Asserts one merged highlight list: sorted, non-overlapping, in bounds.
fn assert_merged(presentation: &artisan_ui::markdown_renderer::InlinePresentation) {
    let highlights = &presentation.highlights;
    let mut previous_end = 0;
    for (range, _) in highlights {
        assert!(
            range.start >= previous_end,
            "runs stay ordered and non-overlapping, got {highlights:?}"
        );
        assert!(
            range.end <= presentation.source.len()
                && presentation.source.is_char_boundary(range.start)
                && presentation.source.is_char_boundary(range.end),
            "run {range:?} escapes the flattened source"
        );
        previous_end = range.end;
    }
}

#[test]
fn merged_highlights_keep_code_bold_and_link_together() {
    // One paragraph carrying all three styles: the old three-chained-call
    // shape would have discarded code and bold, keeping only links.
    let presentation = presented("Use `code`, **bold**, and [label](https://example.invalid/x).\n");
    assert_eq!(presentation.source, "Use code, bold, and label.");
    assert_merged(&presentation);
    assert_eq!(presentation.highlights.len(), 3, "all three runs survive");

    let code = &presentation.highlights[0];
    assert_eq!(&presentation.source[code.0.clone()], "code");
    assert!(
        code.1.background_color.is_some(),
        "code keeps its wash, got {code:?}"
    );
    let bold = &presentation.highlights[1];
    assert_eq!(&presentation.source[bold.0.clone()], "bold");
    assert_eq!(bold.1.font_weight, Some(FontWeight::BOLD));
    let link = &presentation.highlights[2];
    assert_eq!(&presentation.source[link.0.clone()], "label");
    assert!(
        link.1.underline.is_some(),
        "link keeps its underline, got {link:?}"
    );

    assert_eq!(presentation.links.len(), 1);
    assert_eq!(
        presentation.links[0].destination,
        "https://example.invalid/x"
    );
    assert_eq!(presentation.links[0].range, link.0);
}

#[test]
fn nested_bold_code_merges_into_combined_segments() {
    let presentation = presented("A **bold `code` word** here.\n");
    assert_eq!(presentation.source, "A bold code word here.");
    assert_merged(&presentation);

    let code = presentation
        .highlights
        .iter()
        .find(|(range, _)| &presentation.source[range.clone()] == "code")
        .expect("code segment survives inside bold");
    assert_eq!(code.1.font_weight, Some(FontWeight::BOLD));
    assert!(
        code.1.background_color.is_some(),
        "code wash combines with outer bold, got {code:?}"
    );

    let bold_only = presentation
        .highlights
        .iter()
        .find(|(range, _)| &presentation.source[range.clone()] == "bold ")
        .expect("outer bold survives around the code");
    assert_eq!(bold_only.1.font_weight, Some(FontWeight::BOLD));
    assert!(
        bold_only.1.background_color.is_none(),
        "outer bold carries no code wash, got {bold_only:?}"
    );
}

#[test]
fn nested_bold_link_label_combines_underline_and_weight() {
    let presentation =
        presented("Open [**runbook**](https://example.invalid/runbook) now.\n");
    assert_merged(&presentation);
    assert_eq!(presentation.links.len(), 1);
    assert_eq!(
        presentation.links[0].destination,
        "https://example.invalid/runbook"
    );

    let label = presentation
        .highlights
        .iter()
        .find(|(range, _)| &presentation.source[range.clone()] == "runbook")
        .expect("bold link label survives as one segment");
    assert_eq!(label.1.font_weight, Some(FontWeight::BOLD));
    assert!(
        label.1.underline.is_some(),
        "link underline combines with bold, got {label:?}"
    );
    assert_eq!(label.0, presentation.links[0].range);
}

#[test]
fn emphasis_presents_as_italic() {
    let presentation = presented("A *soft* word.\n");
    assert_merged(&presentation);
    let soft = presentation
        .highlights
        .iter()
        .find(|(range, _)| &presentation.source[range.clone()] == "soft")
        .expect("emphasis run survives");
    assert_eq!(soft.1.font_style, Some(FontStyle::Italic));
}

#[test]
fn relative_and_unsafe_links_expose_no_click_metadata() {
    // Relative destinations need a project base the renderer does not own,
    // so they keep their plain label with no highlight and no metadata;
    // unsafe schemes stay inert the same way.
    for source in [
        "See [local](/local/path) here.\n",
        "See [frag](#anchor) here.\n",
        "See [page](guide/intro) here.\n",
        "Catch [me](javascript:alert(1)) here.\n",
        "Take [file](data:text/plain,hi) here.\n",
    ] {
        let presentation = presented(source);
        assert_merged(&presentation);
        assert!(
            presentation.links.is_empty(),
            "no click metadata for {source:?}, got {:?}",
            presentation.links
        );
        assert!(
            presentation.highlights.is_empty(),
            "no link affordance for {source:?}, got {:?}",
            presentation.highlights
        );
    }
}

#[test]
fn mailto_links_open_like_http() {
    let presentation = presented("Write [us](mailto:crew@example.invalid) today.\n");
    assert_merged(&presentation);
    assert_eq!(presentation.links.len(), 1);
    assert_eq!(presentation.links[0].destination, "mailto:crew@example.invalid");
    let label = &presentation.highlights[0];
    assert_eq!(&presentation.source[label.0.clone()], "us");
    assert!(label.1.underline.is_some());
}
