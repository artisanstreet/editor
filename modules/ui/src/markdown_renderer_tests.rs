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
