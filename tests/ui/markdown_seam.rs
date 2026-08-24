//! External coverage for the Phase 1 Markdown engine seam in `artisan_ui`.
//!
//! These tests exercise only the public `artisan_ui::markdown` API and pin
//! the four behaviors this packet requires: raw HTML inertness, open-fence
//! fallback, closed-fence highlighting over byte ranges, and deterministic
//! owned output.

use artisan_ui::markdown::{Block, CodeFence, CodeTokenKind, MarkdownEngine, Span};

const CLOSED_RUST_FENCE: &str =
    "// leading\nfn main() {\n    let message = \"artisan\";\n    let count = 42;\n}\n";

/// Builds a fresh engine; construction is fallible by contract.
fn engine() -> MarkdownEngine {
    MarkdownEngine::new().expect("markdown engine construction must succeed")
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
