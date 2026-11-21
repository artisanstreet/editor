use aqua_text::document::{
    SelectionDocument, SelectionDocumentId, SelectionSourceId, SourcePosition,
};
use editor_aqua::ui::markdown::Span;
use editor_aqua::ui::markdown::visual::{
    BodyTone, LeafError, LinkAction, prepare_inline, prose_leaf,
};
use editor_aqua::ui::theme::{ArtisanTheme, ThemeMode};

#[test]
fn prose_leaf_keeps_source_order_and_separators_for_cross_leaf_copy() {
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    let first = prose_leaf(
        &[Span::Text("one é".into())],
        theme,
        BodyTone::Foreground,
        SelectionDocumentId(7),
        SelectionSourceId(10),
        vec![0],
        "",
    )
    .unwrap();
    let second = prose_leaf(
        &[Span::Html("<raw>".into())],
        theme,
        BodyTone::Foreground,
        SelectionDocumentId(7),
        SelectionSourceId(20),
        vec![1],
        "\n\n",
    )
    .unwrap();
    let mut document = SelectionDocument::new();
    document.register(first.source).unwrap();
    document.register(second.source).unwrap();
    assert!(document.set_selection(
        SourcePosition {
            source: SelectionSourceId(10),
            offset: 4
        },
        SourcePosition {
            source: SelectionSourceId(20),
            offset: 5
        }
    ));
    assert_eq!(document.selected_text().as_deref(), Some("é\n\n<raw>"));
}

#[test]
fn unsupported_inline_interactions_do_not_silently_become_plain_text() {
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    let leaf = prose_leaf(
        &[Span::Strong(vec![Span::Text("important".into())])],
        theme,
        BodyTone::Muted,
        SelectionDocumentId(1),
        SelectionSourceId(1),
        vec![0],
        "",
    );
    assert!(matches!(leaf, Err(LeafError::StyledRuns)));
}

#[test]
fn mixed_inline_keeps_nested_styles_links_and_copy_substitutions() {
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    let prepared = prepare_inline(
        &[
            Span::Text("color #ff0000 ".into()),
            Span::Strong(vec![Span::Code("value".into())]),
            Span::Text(" ".into()),
            Span::Link {
                label: vec![Span::Emphasis(vec![Span::Text("link".into())])],
                destination: "https://example.com".into(),
            },
        ],
        theme,
        SelectionSourceId(4),
        vec![0],
        "",
    );
    let code = prepared
        .runs
        .iter()
        .find(|run| {
            prepared
                .source
                .text
                .get(run.range.clone())
                .expect("the test text slices on a char boundary")
                == "value"
        })
        .unwrap();
    assert_eq!(code.style.weight, Some(aqua_text::FontWeight::NORMAL));
    assert_eq!(code.style.color, Some(theme.colors.text_muted));
    assert!(code.style.monospace);
    assert_eq!(prepared.links[0].destination, "https://example.com");
    assert_eq!(
        prepared
            .source
            .text
            .get(prepared.interactions[0].range.clone())
            .expect("the test text slices on a char boundary"),
        "link"
    );
    let len = prepared.source.text.len();
    let mut document = SelectionDocument::new();
    document.register(prepared.source).unwrap();
    assert!(document.set_selection(
        SourcePosition {
            source: SelectionSourceId(4),
            offset: 0
        },
        SourcePosition {
            source: SelectionSourceId(4),
            offset: len
        },
    ));
    assert_eq!(
        document.selected_text().as_deref(),
        Some("color #ff0000 value link")
    );
}

#[test]
fn inline_actions_preserve_reference_lines_and_color_menu_values() {
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    let prepared = prepare_inline(
        &[
            Span::Link {
                label: vec![Span::Text("file".into())],
                destination: "docs/report.md:42:3".into(),
            },
            Span::Text(" #ff0000".into()),
            Span::Link {
                label: vec![Span::Text("web".into())],
                destination: "https://example.com".into(),
            },
        ],
        theme,
        SelectionSourceId(8),
        vec![0],
        "",
    );
    let Some(LinkAction::OpenReference(reference)) = prepared.link_action(0, None) else {
        panic!("expected file reference")
    };
    assert_eq!(reference.path, "docs/report.md");
    assert_eq!(reference.line, Some(42));
    assert_eq!(
        prepared.link_action(1, None),
        Some(LinkAction::OpenUrl("https://example.com".into()))
    );
    let menu = prepared
        .color_action(
            prepared.colors[0].range.start,
            aqua_core::Point::new(3.0, 4.0),
        )
        .unwrap();
    assert_eq!(menu.entries[0].text, "#ff0000");
    assert_eq!(menu.entries.len(), 7);
    assert!(
        prepared
            .color_action(prepared.source.text.len(), aqua_core::Point::new(0.0, 0.0))
            .is_none()
    );
}

#[test]
fn windows_drive_links_open_the_file_at_their_line() {
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    let prepared = prepare_inline(
        &[
            Span::Link {
                label: vec![Span::Text("main".into())],
                destination: r"C:\src\main.rs:12".into(),
            },
            Span::Link {
                label: vec![Span::Text("lib".into())],
                destination: "d:/src/lib.rs".into(),
            },
        ],
        theme,
        SelectionSourceId(9),
        vec![0],
        "",
    );
    let Some(LinkAction::OpenReference(main)) = prepared.link_action(0, None) else {
        panic!("a backslash drive path opens as a file")
    };
    assert_eq!(
        (main.path.as_str(), main.line),
        (r"C:\src\main.rs", Some(12))
    );
    let Some(LinkAction::OpenReference(lib)) = prepared.link_action(1, None) else {
        panic!("a forward-slash drive path opens as a file")
    };
    assert_eq!((lib.path.as_str(), lib.line), ("d:/src/lib.rs", None));
}
