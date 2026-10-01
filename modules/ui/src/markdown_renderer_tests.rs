use super::*;
use crate::theme::ThemeMode;
use std::collections::HashMap;

struct TestTitles(HashMap<&'static str, &'static str>);

impl RichLinkTitleSource for TestTitles {
    fn resolved_title(&self, destination: &str) -> Option<SharedString> {
        self.0
            .get(destination)
            .map(|title| SharedString::from(*title))
    }
}

fn link_spans(destination: &str, label: &str) -> Vec<Span> {
    vec![
        Span::Text("see ".to_owned()),
        Span::Link {
            label: vec![Span::Text(label.to_owned())],
            destination: destination.to_owned(),
        },
        Span::Text(" end".to_owned()),
    ]
}

#[test]
fn reply_body_tone_is_foreground_detail_body_stays_muted() {
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    assert_eq!(
        markdown_body_text_color(MarkdownBodyTone::Foreground, theme),
        theme.colors.foreground
    );
    assert_eq!(
        markdown_body_text_color(MarkdownBodyTone::Muted, theme),
        theme.colors.muted_foreground
    );
}

#[test]
fn resolved_title_replaces_label_and_keeps_destination_openable() {
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    let titles = TestTitles(HashMap::from([(
        "https://example.com/page",
        "Resolved Page",
    )]));
    let presentation = present_inline_with_titles(
        &link_spans("https://example.com/page", "authored label"),
        theme,
        &titles,
    );
    assert_eq!(presentation.source, "see Resolved Page end");
    assert_eq!(presentation.links.len(), 1);
    assert_eq!(
        presentation.links[0].destination,
        "https://example.com/page"
    );
    assert_eq!(
        &presentation.source[presentation.links[0].range.clone()],
        "Resolved Page"
    );
    assert!(presentation.highlights.iter().any(|(range, style)| {
        range == &presentation.links[0].range && *style == link_style(&theme)
    }));
    assert!(presentation.code_ranges.is_empty());
}

#[test]
fn unresolved_or_failed_links_keep_the_authored_label() {
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    let spans = link_spans("https://example.com/pending", "authored label");

    // Pending and failed resolutions are simply absent from the lookup.
    let empty = TestTitles(HashMap::new());
    let presentation = present_inline_with_titles(&spans, theme, &empty);
    assert_eq!(presentation.source, "see authored label end");
    assert_eq!(
        presentation.links[0].destination,
        "https://example.com/pending"
    );
    assert_eq!(
        &presentation.source[presentation.links[0].range.clone()],
        "authored label"
    );

    // A title for a different URL never leaks into this link.
    let unrelated = TestTitles(HashMap::from([("https://example.com/other", "Other Page")]));
    let presentation = present_inline_with_titles(&spans, theme, &unrelated);
    assert_eq!(presentation.source, "see authored label end");

    // mailto stays an openable authored label even when a lookup names it.
    let mailto = TestTitles(HashMap::from([("mailto:user@example.com", "Email")]));
    let presentation = present_inline_with_titles(
        &link_spans("mailto:user@example.com", "user@example.com"),
        theme,
        &mailto,
    );
    assert_eq!(presentation.source, "see user@example.com end");
    assert_eq!(presentation.links.len(), 1);
}

#[test]
fn resolved_title_replaces_a_formatted_label_wholesale() {
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    let titles = TestTitles(HashMap::from([(
        "https://example.com/page",
        "Resolved Page",
    )]));
    let spans = vec![
        Span::Text("read ".to_owned()),
        Span::Link {
            label: vec![Span::Strong(vec![Span::Text("bold label".to_owned())])],
            destination: "https://example.com/page".to_owned(),
        },
    ];
    let presentation = present_inline_with_titles(&spans, theme, &titles);
    assert_eq!(presentation.source, "read Resolved Page");
    assert_eq!(
        &presentation.source[presentation.links[0].range.clone()],
        "Resolved Page"
    );

    // Relative links stay inert labels and never consult the lookup.
    let relative =
        present_inline_with_titles(&link_spans("/docs/page", "relative label"), theme, &titles);
    assert_eq!(relative.source, "see relative label end");
    assert!(relative.links.is_empty());
}

#[test]
fn blank_resolved_title_never_erases_the_authored_label() {
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    let titles = TestTitles(HashMap::from([("https://example.com/page", "   ")]));
    let presentation = present_inline_with_titles(
        &link_spans("https://example.com/page", "authored label"),
        theme,
        &titles,
    );
    assert_eq!(presentation.source, "see authored label end");
}

#[test]
fn empty_lookup_matches_plain_presentation() {
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    let spans = link_spans("https://example.com/page", "authored label");
    let plain = present_inline(&spans, theme);
    let empty = present_inline_with_titles(&spans, theme, &NoRichLinkTitles);
    assert_eq!(plain, empty);
}

#[test]
fn bare_urls_resolve_titles_without_eating_sentence_punctuation_or_code() {
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    let titles = TestTitles(HashMap::from([(
        "https://example.com/wiki/Test_(thing)",
        "Page title",
    )]));
    let spans = vec![
        Span::Text(
            "Read (https://example.com/wiki/Test_(thing)), then https://example.org/docs. ".into(),
        ),
        Span::Code("https://example.com/code".into()),
    ];
    let rendered = present_inline_with_titles(&spans, theme, &titles);
    assert_eq!(
        rendered.source,
        "Read (Page title), then https://example.org/docs. https://example.com/code"
    );
    assert_eq!(rendered.links.len(), 2);
    assert_eq!(
        rendered.links[0].destination,
        "https://example.com/wiki/Test_(thing)"
    );
    assert_eq!(rendered.links[1].destination, "https://example.org/docs");
    for link in &rendered.links {
        assert!(
            rendered
                .highlights
                .iter()
                .any(|(range, style)| range.start <= link.range.start
                    && range.end >= link.range.end
                    && style.color == Some(theme.colors.banner_info.to_paint()))
        );
    }
    assert_eq!(
        &rendered.source[rendered.code_ranges[0].clone()],
        "https://example.com/code"
    );
}

#[test]
fn explicit_link_label_is_not_recursively_linkified() {
    let rendered = present_inline(
        &link_spans("https://example.com", "https://other.example"),
        ArtisanTheme::for_mode(ThemeMode::Dark),
    );
    assert_eq!(rendered.links.len(), 1);
    assert_eq!(rendered.links[0].destination, "https://example.com");
}

#[test]
fn unresolved_citations_never_render_private_tokens_and_code_stays_literal() {
    let marker = "\u{e200}cite\u{e202}turn0search0\u{e202}turn0search2\u{e201}";
    let rendered = present_inline(
        &[Span::Text(format!("Fact.{marker} Next."))],
        ArtisanTheme::for_mode(ThemeMode::Dark),
    );
    assert_eq!(rendered.source, "Fact. [source unavailable] Next.");
    assert!(rendered.links.is_empty());
    assert_eq!(
        readable_citations("Fact.\u{e200}cite\u{e202}turn0"),
        "Fact."
    );
    let rendered = present_inline(
        &[Span::Code(marker.into())],
        ArtisanTheme::for_mode(ThemeMode::Dark),
    );
    assert_eq!(rendered.source, marker);
}

fn source_spans(before: &str, label: &str, destination: &str, after: &str) -> Vec<Span> {
    let mut spans = Vec::new();
    if !before.is_empty() {
        spans.push(Span::Text(before.to_owned()));
    }
    spans.push(Span::Link {
        label: vec![Span::Text(label.to_owned())],
        destination: destination.to_owned(),
    });
    if !after.is_empty() {
        spans.push(Span::Text(after.to_owned()));
    }
    spans
}

fn split_body(spans: &[Span], titles: &TestTitles) -> (InlinePresentation, Vec<InlineLink>) {
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    let presentation = present_inline_with_titles(spans, theme, titles);
    split_trailing_source_links(presentation)
}

fn empty_titles() -> TestTitles {
    TestTitles(HashMap::new())
}

#[test]
fn glued_trailing_source_splits_off_the_body() {
    let (body, stripped) = split_body(
        &source_spans(
            "It’s “I Have Forgiven Jesus” by Morrissey.",
            "Source",
            "https://example.com/song",
            "",
        ),
        &empty_titles(),
    );
    assert_eq!(body.source, "It’s “I Have Forgiven Jesus” by Morrissey.");
    assert_eq!(stripped.len(), 1);
    assert_eq!(stripped[0].destination, "https://example.com/song");
    assert!(body.links.is_empty());
    assert!(body.citation_links.is_empty());
}

#[test]
fn spaced_trailing_source_trims_the_body() {
    let (body, stripped) = split_body(
        &source_spans("Fact. ", "Source", "https://example.com/a", ""),
        &empty_titles(),
    );
    assert_eq!(body.source, "Fact.");
    assert_eq!(stripped.len(), 1);
}

#[test]
fn consecutive_trailing_sources_all_split_in_order() {
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    let spans = vec![
        Span::Text("Claims.".to_owned()),
        Span::Link {
            label: vec![Span::Text("Source".to_owned())],
            destination: "https://example.com/a".to_owned(),
        },
        Span::Text(" ".to_owned()),
        Span::Link {
            label: vec![Span::Text("Sources".to_owned())],
            destination: "https://example.com/b".to_owned(),
        },
    ];
    let presentation = present_inline_with_titles(&spans, theme, &empty_titles());
    let (body, stripped) = split_trailing_source_links(presentation);
    assert_eq!(body.source, "Claims.");
    assert_eq!(stripped.len(), 2);
    assert_eq!(stripped[0].destination, "https://example.com/a");
    assert_eq!(stripped[1].destination, "https://example.com/b");
}

#[test]
fn source_label_match_is_case_insensitive() {
    let (body, stripped) = split_body(
        &source_spans("Fact.", "SOURCES", "https://example.com/a", ""),
        &empty_titles(),
    );
    assert_eq!(body.source, "Fact.");
    assert_eq!(stripped.len(), 1);
}

#[test]
fn mid_sentence_source_stays_inline() {
    let (body, stripped) = split_body(
        &source_spans("see ", "Source", "https://example.com/a", " end"),
        &empty_titles(),
    );
    assert_eq!(body.source, "see Source end");
    assert!(stripped.is_empty());
    assert_eq!(body.links.len(), 1);
}

#[test]
fn trailing_titled_link_stays_inline() {
    let (body, stripped) = split_body(
        &source_spans("Read ", "the docs", "https://example.com/docs", ""),
        &empty_titles(),
    );
    assert_eq!(body.source, "Read the docs");
    assert!(stripped.is_empty());
    assert_eq!(body.links.len(), 1);
}

#[test]
fn mailto_source_label_is_never_a_citation() {
    let (body, stripped) = split_body(
        &source_spans("Contact ", "Source", "mailto:user@example.com", ""),
        &empty_titles(),
    );
    assert_eq!(body.source, "Contact Source");
    assert!(stripped.is_empty());
}

#[test]
fn title_substituted_source_still_splits() {
    let titles = TestTitles(HashMap::from([(
        "https://example.com/song",
        "Morrissey — I Have Forgiven Jesus",
    )]));
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    let spans = source_spans(
        "It’s by Morrissey.",
        "Source",
        "https://example.com/song",
        "",
    );
    let presentation = present_inline_with_titles(&spans, theme, &titles);
    assert_eq!(
        presentation.source,
        "It’s by Morrissey.Morrissey — I Have Forgiven Jesus"
    );
    let (body, stripped) = split_trailing_source_links(presentation);
    assert_eq!(body.source, "It’s by Morrissey.");
    assert_eq!(stripped.len(), 1);
    assert_eq!(stripped[0].destination, "https://example.com/song");
}

#[test]
fn stripped_body_keeps_only_ranges_it_addresses() {
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    let spans = vec![
        Span::Strong(vec![Span::Text("Bold claim".to_owned())]),
        Span::Text(" with proof.".to_owned()),
        Span::Link {
            label: vec![Span::Text("Source".to_owned())],
            destination: "https://example.com/a".to_owned(),
        },
    ];
    let presentation = present_inline_with_titles(&spans, theme, &empty_titles());
    let (body, stripped) = split_trailing_source_links(presentation);
    assert_eq!(body.source, "Bold claim with proof.");
    assert_eq!(stripped.len(), 1);
    let end = body.source.len();
    assert!(
        body.highlights
            .iter()
            .all(|(range, _)| range.end <= end && range.start < range.end)
    );
    assert!(body.links.iter().all(|link| link.range.end <= end));
    assert!(body.code_ranges.iter().all(|range| range.end <= end));
    assert!(body.source.is_char_boundary(end));
}

#[test]
fn citation_host_extraction_prefers_bare_host() {
    assert_eq!(
        citation_host("https://example.com/a?x=1#frag").as_deref(),
        Some("example.com")
    );
    assert_eq!(
        citation_host("https://www.example.com/").as_deref(),
        Some("example.com")
    );
    assert_eq!(
        citation_host("http://user:secret@example.com:8080/p").as_deref(),
        Some("example.com")
    );
    assert!(citation_host("https://").is_none());
    assert!(citation_host("not a url").is_none());
}

#[test]
fn fence_display_drops_only_the_final_line_terminator() {
    assert_eq!(fence_display_source("one\n"), "one");
    assert_eq!(fence_display_source("one\r\n"), "one");
    assert_eq!(fence_display_source("one\ntwo\n"), "one\ntwo");
    // A blank line the author wrote stays.
    assert_eq!(fence_display_source("one\n\n"), "one\n");
    // A streaming fence may end mid-line.
    assert_eq!(fence_display_source("one"), "one");
    assert_eq!(fence_display_source(""), "");
}

// ---------------------------------------------------------------------------
// Prepared presentation: no per-frame work, and output identical to the
// per-frame flattener it replaced.
// ---------------------------------------------------------------------------

/// The resolved-style flattener as it ran on every frame before the prepared
/// presentation, kept verbatim as the oracle the cached path must match.
mod legacy {
    use super::super::{
        InlineLink, InlinePresentation, RichLinkTitleSource, bare_url_spans, code_style,
        emphasis_style, is_openable_link_destination, is_rich_link_destination,
        is_source_attribution_label, link_style, readable_citations, span_label_text, strong_style,
    };
    use crate::markdown::Span;
    use crate::theme::ArtisanTheme;
    use gpui::HighlightStyle;
    use std::ops::Range;

    #[derive(Default)]
    struct Accumulator {
        source: String,
        runs: Vec<(Range<usize>, HighlightStyle)>,
        links: Vec<InlineLink>,
        citation_links: Vec<InlineLink>,
        code_ranges: Vec<Range<usize>>,
        icon_offsets: Vec<(usize, String)>,
    }

    fn emit_run(accumulator: &mut Accumulator, start: usize, end: usize, style: HighlightStyle) {
        if start >= end || style == HighlightStyle::default() {
            return;
        }
        if let Some((last_range, last_style)) = accumulator.runs.last_mut()
            && *last_style == style
            && last_range.end == start
        {
            last_range.end = end;
            return;
        }
        accumulator.runs.push((start..end, style));
    }

    pub(super) fn present(
        spans: &[Span],
        theme: ArtisanTheme,
        titles: &dyn RichLinkTitleSource,
    ) -> InlinePresentation {
        let mut accumulator = Accumulator::default();
        flatten(
            spans,
            HighlightStyle::default(),
            &mut accumulator,
            &theme,
            titles,
            false,
        );
        InlinePresentation {
            source: accumulator.source,
            highlights: accumulator.runs,
            links: accumulator.links,
            citation_links: accumulator.citation_links,
            code_ranges: accumulator.code_ranges,
            icon_offsets: accumulator.icon_offsets,
        }
    }

    fn flatten(
        spans: &[Span],
        inherited: HighlightStyle,
        accumulator: &mut Accumulator,
        theme: &ArtisanTheme,
        titles: &dyn RichLinkTitleSource,
        in_link: bool,
    ) {
        for span in spans {
            match span {
                Span::Text(inline) | Span::Html(inline) => {
                    let cleaned;
                    let inline = if inline.contains('\u{e200}') {
                        cleaned = readable_citations(inline);
                        &cleaned
                    } else {
                        inline
                    };
                    if matches!(span, Span::Text(_))
                        && !in_link
                        && let Some(linked) = bare_url_spans(inline)
                    {
                        flatten(&linked, inherited, accumulator, theme, titles, true);
                        continue;
                    }
                    let start = accumulator.source.len();
                    accumulator.source.push_str(inline);
                    emit_run(accumulator, start, accumulator.source.len(), inherited);
                }
                Span::Code(code) => {
                    let start = accumulator.source.len();
                    accumulator.source.push_str(code);
                    let end = accumulator.source.len();
                    if start < end {
                        accumulator.code_ranges.push(start..end);
                    }
                    emit_run(
                        accumulator,
                        start,
                        end,
                        inherited.highlight(code_style(theme, in_link)),
                    );
                }
                Span::Emphasis(inner) => flatten(
                    inner,
                    inherited.highlight(emphasis_style()),
                    accumulator,
                    theme,
                    titles,
                    in_link,
                ),
                Span::Strong(inner) => flatten(
                    inner,
                    inherited.highlight(strong_style(theme, in_link)),
                    accumulator,
                    theme,
                    titles,
                    in_link,
                ),
                Span::Link { label, destination } => {
                    if is_openable_link_destination(destination) {
                        let start = accumulator.source.len();
                        let source_attribution = is_rich_link_destination(destination)
                            && is_source_attribution_label(&span_label_text(label));
                        let resolved = is_rich_link_destination(destination)
                            .then(|| titles.resolved_title(destination))
                            .flatten()
                            .filter(|title| !title.trim().is_empty());
                        if titles.favicon(destination).is_some() {
                            accumulator.icon_offsets.push((start, destination.clone()));
                            accumulator.source.push_str("\u{2003}\u{2060}\u{00a0}");
                        }
                        if let Some(title) = resolved {
                            accumulator.source.push_str(title.as_ref());
                            emit_run(
                                accumulator,
                                start,
                                accumulator.source.len(),
                                inherited.highlight(link_style(theme)),
                            );
                        } else {
                            flatten(
                                label,
                                inherited.highlight(link_style(theme)),
                                accumulator,
                                theme,
                                titles,
                                true,
                            );
                        }
                        let end = accumulator.source.len();
                        if start < end {
                            let link = InlineLink {
                                range: start..end,
                                destination: destination.clone(),
                            };
                            if source_attribution {
                                accumulator.citation_links.push(link.clone());
                            }
                            accumulator.links.push(link);
                        }
                    } else {
                        flatten(label, inherited, accumulator, theme, titles, in_link);
                    }
                }
            }
        }
    }

    /// The old per-frame trailing-citation split over resolved runs.
    pub(super) fn split(
        mut presentation: InlinePresentation,
    ) -> (InlinePresentation, Vec<InlineLink>) {
        let mut stripped: Vec<InlineLink> = Vec::new();
        loop {
            let trimmed_len = presentation.source.trim_end().len();
            let qualifies = presentation
                .citation_links
                .last()
                .is_some_and(|last| last.range.end == trimmed_len);
            if !qualifies {
                break;
            }
            let Some(link) = presentation.citation_links.pop() else {
                break;
            };
            presentation.source.truncate(link.range.start);
            stripped.push(link);
        }
        stripped.reverse();
        if stripped.is_empty() {
            return (presentation, stripped);
        }
        let end = presentation.source.trim_end().len();
        presentation.source.truncate(end);
        presentation.highlights.retain_mut(|(range, _)| {
            if range.start >= end {
                false
            } else {
                range.end = range.end.min(end);
                true
            }
        });
        presentation.code_ranges.retain_mut(|range| {
            if range.start >= end {
                false
            } else {
                range.end = range.end.min(end);
                true
            }
        });
        presentation.links.retain(|link| link.range.end <= end);
        presentation
            .citation_links
            .retain(|link| link.range.end <= end);
        presentation
            .icon_offsets
            .retain(|(offset, _)| *offset < end);
        (presentation, stripped)
    }
}

/// Bodies covering every inline construct, nesting shape, and block kind the
/// prepared presentation caches.
const CORPUS: &[&str] = &[
    "# Title with **bold** and `code`\n\nPlain paragraph.",
    "Para with *em*, **strong *nested em***, ***both***, **a __b__ c**, and **x****y**.",
    "Links: [plain](https://example.com/a), [**bold** `code` *em*](https://example.com/b), \
     bare https://example.org/x. and (https://example.com/wiki/T_(x)), \
     [mail](mailto:a@b.c), [rel](./x), [ftp](ftp://h/p).",
    "Claim with proof.[Source](https://example.com/s) [Sources](https://example.com/t)",
    "see [Source](https://example.com/s). Next.",
    "- item **one**\n- [ ] task with `code`\n- [x] done\n  1. nested [Source](https://src.example/1)\n  2. two\n\n3. three\n4. four",
    "| a | **b** | c |\n|---|:-:|--:|\n| `c` | [d](https://example.com/a) | https://e.example/f |\n| short |",
    "```rust\n// comment\nfn main() { let x = \"s\"; let n = 42; }\n```\n\n```\nbare fence\n```\n\n    indented\n\n```python\nprint('x')",
    "<div>block html</div>\n\nInline <span>html</span> and \u{e200}cite\u{e202}turn0\u{e201} marker, \u{e200}unfinished",
    "1. \n2. empty items\n\n## Heading `code` [link](https://example.com/a)",
];

fn corpus_titles() -> TestTitles {
    TestTitles(HashMap::from([
        ("https://example.com/a", "Resolved A"),
        ("https://example.com/s", "Source title"),
        ("https://e.example/f", "Bare resolved"),
    ]))
}

/// Titles plus a favicon for one destination, so icon slots are covered.
struct IconTitles {
    titles: TestTitles,
    icon: std::sync::Arc<gpui::RenderImage>,
}

impl RichLinkTitleSource for IconTitles {
    fn favicon(&self, destination: &str) -> Option<std::sync::Arc<gpui::RenderImage>> {
        (destination == "https://example.com/b").then(|| self.icon.clone())
    }

    fn resolved_title(&self, destination: &str) -> Option<SharedString> {
        self.titles.resolved_title(destination)
    }
}

fn icon_titles() -> IconTitles {
    IconTitles {
        titles: corpus_titles(),
        icon: std::sync::Arc::new(gpui::RenderImage::new(Vec::new())),
    }
}

/// Every inline span slice of a document in render order, with nested items
/// and table cells.
fn inline_slices(blocks: &[Block]) -> Vec<&[Span]> {
    let mut slices = Vec::new();
    for block in blocks {
        match block {
            Block::Heading { spans, .. } | Block::Paragraph { spans, .. } => {
                slices.push(&spans[..])
            }
            Block::List { items, .. } => {
                for item in items {
                    slices.extend(inline_slices(&item.blocks));
                }
            }
            Block::Table(table) => {
                for cell in table.header.iter().chain(table.rows.iter().flatten()) {
                    slices.push(&cell.spans[..]);
                }
            }
            Block::Code(_) | Block::Html { .. } => {}
        }
    }
    slices
}

/// Every prepared leaf in the same order as [`inline_slices`] (empty-item
/// leaves are skipped: they have no authored spans).
fn prepared_leaves(blocks: &PreparedBlocks) -> Vec<&PreparedLeaf> {
    let mut leaves = Vec::new();
    for block in &blocks.blocks {
        match &block.kind {
            PreparedKind::Heading { leaf, .. } | PreparedKind::Paragraph { leaf } => {
                leaves.push(leaf);
            }
            PreparedKind::List(list) => {
                for item in &list.items {
                    leaves.extend(prepared_leaves(&item.blocks));
                }
            }
            PreparedKind::Table(table) => {
                for cell in table.rows.iter().flatten() {
                    leaves.extend(cell.leaf.as_ref());
                }
            }
            PreparedKind::Code(_) | PreparedKind::Html { .. } => {}
        }
    }
    leaves
}

fn assert_leaf_matches_legacy(
    spans: &[Span],
    leaf: &PreparedLeaf,
    theme: ArtisanTheme,
    titles: &dyn RichLinkTitleSource,
    counters: &MarkdownWorkCounters,
) {
    let (expected, expected_citations) = legacy::split(legacy::present(spans, theme, titles));
    let (presentation, icons) = leaf.presentation(spans, titles, counters);
    let (highlights, overrides) = presentation.resolved(&theme);
    assert_eq!(
        presentation.text.as_ref(),
        expected.source,
        "text for {spans:?}"
    );
    assert_eq!(
        &highlights[..],
        &expected.highlights[..],
        "runs for {spans:?}"
    );
    assert_eq!(
        presentation.link_ranges.to_vec(),
        expected
            .links
            .iter()
            .map(|link| link.range.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        presentation
            .link_destinations
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        expected
            .links
            .iter()
            .map(|link| link.destination.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        overrides
            .iter()
            .map(|entry| entry.range.clone())
            .collect::<Vec<_>>(),
        expected.code_ranges
    );
    assert!(overrides.iter().all(|entry| {
        entry.font_family.as_deref() == Some(theme.typography.mono.family)
            && entry.letter_spacing == Some(px(0.0))
    }));
    assert_eq!(presentation.icon_offsets.to_vec(), expected.icon_offsets);
    assert_eq!(
        icons.iter().map(|(offset, _)| *offset).collect::<Vec<_>>(),
        expected
            .icon_offsets
            .iter()
            .filter(|(_, destination)| titles.favicon(destination).is_some())
            .map(|(offset, _)| *offset)
            .collect::<Vec<_>>()
    );
    assert_eq!(presentation.citations.to_vec(), expected_citations);
}

#[test]
fn prepared_presentation_matches_the_per_frame_flattener() {
    let engine = MarkdownEngine::new().expect("the built-in engine constructs");
    let counters = MarkdownWorkCounters::default();
    let plain = NoRichLinkTitles;
    let titled = corpus_titles();
    let icons = icon_titles();
    let sources: [&dyn RichLinkTitleSource; 3] = [&plain, &titled, &icons];
    for body in CORPUS {
        let document = Rc::new(engine.parse_document(body).expect("corpus parses"));
        let prepared = PreparedMarkdown::new(Rc::clone(&document), Rc::default());
        let slices = inline_slices(document.blocks());
        let leaves = prepared_leaves(prepared.root());
        assert_eq!(slices.len(), leaves.len(), "leaf count for {body:?}");
        for mode in [ThemeMode::Dark, ThemeMode::Light] {
            let theme = ArtisanTheme::for_mode(mode);
            for titles in sources {
                for (spans, leaf) in slices.iter().zip(&leaves) {
                    assert_leaf_matches_legacy(spans, leaf, theme, titles, &counters);
                    // The public presentation seam matches the oracle too.
                    let public = present_inline_with_titles(spans, theme, titles);
                    assert_eq!(public, legacy::present(spans, theme, titles));
                }
            }
        }
    }
}

/// Every fence of a document with its prepared counterpart.
fn fence_pairs<'a>(
    blocks: &'a [Block],
    prepared: &'a PreparedBlocks,
) -> Vec<(&'a crate::markdown::CodeFence, &'a PreparedCode)> {
    let mut pairs = Vec::new();
    for (block, prepared) in blocks.iter().zip(&prepared.blocks) {
        match (block, &prepared.kind) {
            (Block::Code(fence), PreparedKind::Code(code)) => pairs.push((fence, code)),
            (Block::List { items, .. }, PreparedKind::List(list)) => {
                for (item, prepared_item) in items.iter().zip(&list.items) {
                    pairs.extend(fence_pairs(&item.blocks, &prepared_item.blocks));
                }
            }
            _ => {}
        }
    }
    pairs
}

#[test]
fn prepared_fences_match_the_per_frame_highlights() {
    let engine = MarkdownEngine::new().expect("the built-in engine constructs");
    let body = "```rust\n// c\nfn main() { let s = \"x\"; }\n```\n\n- item\n\n  ```rust\n  let n = 1;\n  ```\n\n```\nplain\n```";
    let document = Rc::new(engine.parse_document(body).expect("fences parse"));
    let prepared = PreparedMarkdown::new(Rc::clone(&document), Rc::default());
    let pairs = fence_pairs(document.blocks(), prepared.root());
    assert_eq!(pairs.len(), 3);
    for mode in [ThemeMode::Dark, ThemeMode::Light] {
        let theme = ArtisanTheme::for_mode(mode);
        for (fence, code) in &pairs {
            let display = fence_display_source(&fence.source);
            let expected = fence
                .tokens
                .as_deref()
                .unwrap_or_default()
                .iter()
                .filter_map(|token| valid_code_range(token, display))
                .map(|(range, kind)| (range, code_token_style(&theme, kind)))
                .collect::<Vec<_>>();
            assert_eq!(code.fence.source.as_ref(), display);
            assert_eq!(code.fence.multiline, display.contains('\n'));
            assert_eq!(&code.fence.highlights(&theme)[..], &expected[..]);
        }
    }
}

#[test]
fn element_ids_match_the_per_frame_selectors() {
    let engine = MarkdownEngine::new().expect("the built-in engine constructs");
    let body = "Para\n\n```rust\nfn a() {}\n```\n\n- item\n  - nested\n\n| a |\n|---|\n| b |";
    let document = Rc::new(engine.parse_document(body).expect("body parses"));
    let prepared = PreparedMarkdown::new(document, Rc::default());
    let ids = prepared.ids(&SharedString::from("msg"));
    let root = prepared.root();
    let id = |slot: Slot| ids[slot].to_string();
    assert_eq!(id(ROOT_SLOT), "msg-markdown");
    assert_eq!(id(root.blocks[0].selector), "msg-markdown-block-0");
    let PreparedKind::Code(code) = &root.blocks[1].kind else {
        panic!("second block is the fence");
    };
    assert_eq!(id(code.code), "msg-markdown-block-1-code");
    assert_eq!(id(code.text), "msg-markdown-block-1-code-text");
    assert_eq!(id(code.copy), "msg-markdown-block-1-code-copy");
    assert_eq!(id(code.copy_state), "msg-markdown-block-1-code-copy-state");
    let PreparedKind::List(list) = &root.blocks[2].kind else {
        panic!("third block is the list");
    };
    assert_eq!(id(list.list), "msg-markdown-block-2-list");
    assert_eq!(id(list.items[0].selector), "msg-markdown-block-2-item-0");
    let PreparedKind::List(nested) = &list.items[0].blocks.blocks[1].kind else {
        panic!("the item nests a list");
    };
    assert_eq!(
        id(nested.items[0].selector),
        "msg-markdown-block-2-item-0-block-1-item-0"
    );
    let PreparedKind::Table(table) = &root.blocks[3].kind else {
        panic!("fourth block is the table");
    };
    assert_eq!(id(table.table), "msg-markdown-block-3-table");
    assert_eq!(
        id(table.rows[1][0].selector),
        "msg-markdown-block-3-table-r1-c0"
    );
    assert_eq!(
        id(table.rows[1][0].content),
        "msg-markdown-block-3-table-r1-c0-content"
    );
    // The same selector reuses its table; another selector gets its own.
    assert!(Rc::ptr_eq(&ids, &prepared.ids(&SharedString::from("msg"))));
    assert_eq!(
        prepared.ids(&SharedString::from("other"))[ROOT_SLOT],
        "other-markdown"
    );
}

#[test]
fn repeated_render_of_an_unchanged_body_does_no_presentation_work() {
    let renderer = MarkdownRenderer::new();
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    let body = CORPUS.join("\n\n");
    let _ = renderer.render_source(&body, theme, "reply");
    let first = renderer.parse_report();
    assert_eq!(first.parses, 1);
    assert!(first.inline_flattens > 0);
    assert!(first.fence_highlights > 0);

    for _ in 0..3 {
        let _ = renderer.render_source(&body, theme, "reply");
        let _ = renderer.render_source(&body, ArtisanTheme::for_mode(ThemeMode::Light), "reply");
    }
    let repeated = renderer.parse_report();
    assert_eq!(repeated.parses, 1, "{repeated:?}");
    assert_eq!(repeated.hits, 6, "{repeated:?}");
    assert_eq!(
        repeated.inline_flattens, first.inline_flattens,
        "{repeated:?}"
    );
    assert_eq!(
        repeated.fence_highlights, first.fence_highlights,
        "{repeated:?}"
    );

    // A shared body the cache already holds matches by identity.
    let shared = SharedString::from(body.clone());
    let _ = renderer.render_shared_source_with_tone_and_titles(
        &shared,
        theme,
        "reply",
        MarkdownBodyTone::Foreground,
        &NoRichLinkTitles,
    );
    let _ = renderer.render_shared_source_with_tone_and_titles(
        &shared,
        theme,
        "reply",
        MarkdownBodyTone::Foreground,
        &NoRichLinkTitles,
    );
    let identity = renderer.parse_report();
    assert_eq!(identity.parses, 1);
    assert_eq!(identity.hits, 8);
    assert_eq!(identity.inline_flattens, first.inline_flattens);
}

#[test]
fn resolved_titles_reflatten_a_leaf_only_when_they_change() {
    let renderer = MarkdownRenderer::new();
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    let body = "Read [docs](https://example.com/a) today.\n\nNo links here.";
    let empty = TestTitles(HashMap::new());
    let _ = renderer.render_source_with_tone_and_titles(
        body,
        theme,
        "reply",
        MarkdownBodyTone::Foreground,
        &empty,
    );
    let prepared = renderer.parse_report().inline_flattens;
    // Unresolved titles render the prepared presentation.
    let _ = renderer.render_source_with_tone_and_titles(
        body,
        theme,
        "reply",
        MarkdownBodyTone::Foreground,
        &empty,
    );
    assert_eq!(renderer.parse_report().inline_flattens, prepared);

    // A title arriving re-flattens the one leaf that links to it, once.
    let titles = corpus_titles();
    for _ in 0..3 {
        let _ = renderer.render_source_with_tone_and_titles(
            body,
            theme,
            "reply",
            MarkdownBodyTone::Foreground,
            &titles,
        );
    }
    assert_eq!(renderer.parse_report().inline_flattens, prepared + 1);

    // A different answer re-flattens again.
    let changed = TestTitles(HashMap::from([("https://example.com/a", "New title")]));
    let _ = renderer.render_source_with_tone_and_titles(
        body,
        theme,
        "reply",
        MarkdownBodyTone::Foreground,
        &changed,
    );
    assert_eq!(renderer.parse_report().inline_flattens, prepared + 2);
}

#[test]
fn streaming_append_rehighlights_only_the_changed_fence() {
    let renderer = MarkdownRenderer::new();
    let first = "```rust\nfn a() {}\n```\n\n```rust\nfn b() {}\n```\n";
    let _ = renderer.cached_document(first);
    let report = renderer.parse_report();
    assert_eq!(report.fence_highlights, 2);
    assert_eq!(report.fence_highlight_hits, 0);

    // A delta that opens a third fence re-parses the body, but both settled
    // fences reuse their tokens and the open fence is never classified.
    let streaming = format!("{first}\n```rust\nfn c(");
    let _ = renderer.cached_document(&streaming);
    let report = renderer.parse_report();
    assert_eq!(report.parses, 2);
    assert_eq!(report.fence_highlights, 2);
    assert_eq!(report.fence_highlight_hits, 2);

    // Closing it classifies exactly that fence.
    let closed = format!("{first}\n```rust\nfn c() {{}}\n```\n");
    let document = renderer
        .cached_document(&closed)
        .expect("closed body parses");
    let report = renderer.parse_report();
    assert_eq!(report.fence_highlights, 3);
    assert_eq!(report.fence_highlight_hits, 4);

    // Memoized tokens are exactly what a fresh classification produces.
    let fresh = MarkdownEngine::new()
        .expect("the built-in engine constructs")
        .parse_document(&closed)
        .expect("closed body parses");
    assert_eq!(*document, fresh);
}
