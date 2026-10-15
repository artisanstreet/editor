use super::*;
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
fn resolved_title_preserves_authored_label_and_destination() {
    let theme = ArtisanTheme::resolved();
    let presentation = present_inline(
        &link_spans("https://example.com/page", "authored label"),
        theme,
    );
    assert_eq!(presentation.source, "see authored label end");
    assert_eq!(presentation.links.len(), 1);
    assert_eq!(
        presentation.links[0].destination,
        "https://example.com/page"
    );
    assert_eq!(
        &presentation.source[presentation.links[0].range.clone()],
        "authored label"
    );
    assert!(presentation.highlights.iter().any(|(range, style)| {
        range == &presentation.links[0].range && *style == link_style(&theme)
    }));
    assert!(presentation.code_ranges.is_empty());
}

#[test]
fn unresolved_or_failed_links_keep_the_authored_label() {
    let theme = ArtisanTheme::resolved();
    let spans = link_spans("https://example.com/pending", "authored label");

    // Pending and failed resolutions are simply absent from the lookup.
    let presentation = present_inline(&spans, theme);
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
    let presentation = present_inline(&spans, theme);
    assert_eq!(presentation.source, "see authored label end");

    // mailto stays an openable authored label even when a lookup names it.
    let presentation = present_inline(
        &link_spans("mailto:user@example.com", "user@example.com"),
        theme,
    );
    assert_eq!(presentation.source, "see user@example.com end");
    assert_eq!(presentation.links.len(), 1);
}

#[test]
fn resolved_title_preserves_formatted_authored_label() {
    let theme = ArtisanTheme::resolved();
    let spans = vec![
        Span::Text("read ".to_owned()),
        Span::Link {
            label: vec![Span::Strong(vec![Span::Text("bold label".to_owned())])],
            destination: "https://example.com/page".to_owned(),
        },
    ];
    let presentation = present_inline(&spans, theme);
    assert_eq!(presentation.source, "read bold label");
    assert_eq!(
        &presentation.source[presentation.links[0].range.clone()],
        "bold label"
    );

    // Relative links stay inert labels and never consult the lookup.
    let relative = present_inline(&link_spans("/docs/page", "relative label"), theme);
    assert_eq!(relative.source, "see relative label end");
    assert!(relative.links.is_empty());
}

#[test]
fn blank_resolved_title_never_erases_the_authored_label() {
    let theme = ArtisanTheme::resolved();
    let presentation = present_inline(
        &link_spans("https://example.com/page", "authored label"),
        theme,
    );
    assert_eq!(presentation.source, "see authored label end");
}

#[test]
fn bare_urls_keep_authored_text_and_sentence_punctuation() {
    let theme = ArtisanTheme::resolved();
    let spans = vec![
        Span::Text(
            "Read (https://example.com/wiki/Test_(thing)), then https://example.org/docs. ".into(),
        ),
        Span::Code("https://example.com/code".into()),
    ];
    let rendered = present_inline(&spans, theme);
    assert_eq!(
        rendered.source,
        "Read (https://example.com/wiki/Test_(thing)), then https://example.org/docs. https://example.com/code"
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
        ArtisanTheme::resolved(),
    );
    assert_eq!(rendered.links.len(), 1);
    assert_eq!(rendered.links[0].destination, "https://example.com");
}

#[test]
fn source_label_is_ordinary_authored_link_text() {
    let spans = vec![
        Span::Text("Fact. ".into()),
        Span::Link {
            label: vec![Span::Text("Source".into())],
            destination: "https://example.com".into(),
        },
    ];
    let shown = present_inline(&spans, ArtisanTheme::resolved());
    assert_eq!(shown.source, "Fact. Source");
    assert_eq!(shown.links.len(), 1);
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
