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
