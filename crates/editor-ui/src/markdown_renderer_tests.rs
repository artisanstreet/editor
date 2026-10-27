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

    // Relative file links retain their authored labels and are openable.
    let relative = present_inline(&link_spans("docs/page", "relative label"), theme);
    assert_eq!(relative.source, "see relative label end");
    assert_eq!(relative.links[0].destination, "docs/page");
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
                    && style.color == Some(theme.colors.link.to_paint()))
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

#[test]
fn a_message_paints_its_leading_command_and_keeps_its_markdown() {
    let theme = ArtisanTheme::resolved();
    let engine = crate::markdown::MarkdownEngine::new().unwrap();
    let mut document = engine
        .parse_document("/goal ship **it**\n\n## Next")
        .unwrap();
    document.mark_command("/goal");
    let Block::Paragraph { spans, .. } = &document.blocks()[0] else {
        panic!("the message opens with a paragraph");
    };
    let presentation = present_inline(spans, theme);
    assert_eq!(presentation.source, "/goal ship it");
    assert!(
        presentation
            .highlights
            .iter()
            .any(|(range, style)| range == &(0..5) && *style == command_style(&theme)),
        "the command paints as the composer paints it: {:?}",
        presentation.highlights
    );
    assert!(
        presentation
            .highlights
            .iter()
            .any(|(range, _)| &presentation.source[range.clone()] == "it"),
        "the rest stays markdown"
    );
    assert!(matches!(&document.blocks()[1], Block::Heading { .. }));

    // Text that only mentions the command later is left alone.
    let mut document = engine.parse_document("see /goal later").unwrap();
    document.mark_command("/goal");
    let Block::Paragraph { spans, .. } = &document.blocks()[0] else {
        panic!("one paragraph");
    };
    assert_eq!(spans, &vec![Span::Text("see /goal later".to_owned())]);
}

#[test]
fn file_links_show_labels_and_keep_absolute_destinations() {
    let theme = ArtisanTheme::resolved();
    for destination in [
        "/tmp/audit/report.md",
        "/tmp/audit/report.md:42:3",
        "file:///tmp/audit/report.md#L42",
    ] {
        let shown = present_inline(&link_spans(destination, "Full report"), theme);
        assert_eq!(shown.source, "see Full report end");
        assert_eq!(shown.links[0].destination, destination);
        assert_eq!(&shown.source[shown.links[0].range.clone()], "Full report");
        assert!(shown.highlights.iter().any(|(range, style)| {
            range == &shown.links[0].range && *style == link_style(&theme)
        }));
    }
}

#[test]
fn file_links_reserve_icons_without_putting_them_in_copied_text() {
    let flat = inline::flatten_inline_with_swatches(&link_spans(
        "/tmp/hello world/report.ts",
        "Full report",
    ));
    assert_eq!(flat.file_icons.len(), 1);
    assert_eq!(
        flat.file_icons[0].1,
        (
            editor_assets::file_icons::file(std::path::Path::new("report.ts"), false),
            editor_assets::file_icons::file(std::path::Path::new("report.ts"), true)
        )
    );
    let (slot, icon) = (flat.file_slots[0].clone(), flat.file_icons[0].0);
    assert!(slot.contains(&icon));
    assert_eq!(&flat.source[slot.end..], "Full report end");
    assert_eq!(
        &flat.source[flat.links[0].range.clone()],
        format!("{}\u{00a0}Full report", inline::SWATCH_SLOT)
    );
}

#[test]
fn file_uris_decode_spaces_and_reject_remote_authorities() {
    assert_eq!(
        file_link_path("file:///tmp/hello%20world/report.md"),
        Some("/tmp/hello world/report.md".into())
    );
    assert_eq!(
        file_link_path("docs/report.md:42"),
        Some("docs/report.md".into())
    );
    assert_eq!(file_link_line("docs/report.md:42:3"), Some(42));
    assert_eq!(file_link_line("file:///tmp/report.md#L42"), Some(42));
    for destination in [
        "file://other-computer/tmp/report.md",
        "//other-computer/report.md",
        "javascript:alert(1)",
    ] {
        assert!(file_link_path(destination).is_none(), "{destination}");
        assert!(!is_openable_link_destination(destination), "{destination}");
    }
}

struct FileLinkGallery {
    mentions: Option<crate::text_editor::Tokenizer>,
    renderer: MarkdownRenderer,
    prepared: Rc<PreparedMarkdown>,
}
impl gpui::Render for FileLinkGallery {
    fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        self.renderer.render_prepared(
            &self.prepared,
            0,
            &ArtisanTheme::resolved(),
            &"file-links".into(),
            MarkdownBodyTone::Foreground,
            0..self.prepared.block_count(),
            None,
            false,
            self.mentions.as_ref(),
        )
    }
}

#[gpui::test]
fn file_link_icons_paint_in_native_selectable_text(cx: &mut gpui::TestAppContext) {
    let document = crate::markdown::MarkdownEngine::new().unwrap().parse_document(
        "[Full report](/tmp/report.md) · [Query inventory](/tmp/queries.csv) · [Code](/tmp/example.ts)"
    ).unwrap();
    let parts = prepared::PreparedParts::new(&document, None);
    let prepared = PreparedMarkdown::from_indexed_parts(&document, parts, None);
    let (_, cx) = cx.add_window_view(|_, _| FileLinkGallery {
        mentions: None,
        renderer: MarkdownRenderer::new(),
        prepared: Rc::new(prepared),
    });
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
    let bounds = cx.debug_bounds("file-links-md-0").unwrap();
    assert!(bounds.size.height > px(0.0));
    cx.simulate_click(
        bounds.origin + gpui::point(px(8.0), px(12.0)),
        gpui::Modifiers::none(),
    );
    for key in ["ctrl-a", "ctrl-c"] {
        cx.update(|window, cx| {
            window.dispatch_event(
                gpui::PlatformInput::KeyDown(gpui::KeyDownEvent {
                    keystroke: gpui::Keystroke::parse(key).unwrap(),
                    is_held: false,
                    prefer_character_input: false,
                }),
                cx,
            );
        });
        cx.run_until_parked();
    }
    let copied = cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text()));
    assert_eq!(
        copied.as_deref(),
        Some("Full report · Query inventory · Code")
    );
}

#[gpui::test]
fn skill_glyphs_preserve_the_sigil_when_selected_and_copied(cx: &mut gpui::TestAppContext) {
    let source = "界 $review then $review.";
    let document = crate::markdown::MarkdownEngine::new()
        .unwrap()
        .parse_document(source)
        .unwrap();
    let parts = prepared::PreparedParts::new(&document, None);
    let prepared = PreparedMarkdown::from_indexed_parts(&document, parts, None);
    let (_, cx) = cx.add_window_view(|_, _| FileLinkGallery {
        renderer: MarkdownRenderer::new(),
        prepared: Rc::new(prepared),
        mentions: Some(Rc::new(|text| {
            text.match_indices("$review")
                .map(|(start, name)| crate::text_editor::Token {
                    range: start..start + name.len(),
                    glyph: Some(crate::AssetId::TABLER_FILE_SPARK),
                    style: HighlightStyle {
                        color: Some(ArtisanTheme::resolved().colors.mention_skill.to_paint()),
                        font_weight: Some(FontWeight::MEDIUM),
                        ..Default::default()
                    },
                })
                .collect()
        })),
    });
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
    let bounds = cx.debug_bounds("file-links-md-0").unwrap();
    cx.simulate_click(
        bounds.origin + gpui::point(px(8.0), px(12.0)),
        gpui::Modifiers::none(),
    );
    for key in ["ctrl-a", "ctrl-c"] {
        cx.update(|window, cx| {
            window.dispatch_event(
                gpui::PlatformInput::KeyDown(gpui::KeyDownEvent {
                    keystroke: gpui::Keystroke::parse(key).unwrap(),
                    is_held: false,
                    prefer_character_input: false,
                }),
                cx,
            );
        });
        cx.run_until_parked();
    }
    let copied = cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text()));
    assert_eq!(copied.as_deref(), Some(source));
}
