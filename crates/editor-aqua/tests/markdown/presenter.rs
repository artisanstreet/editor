use aqua_text::document::{SelectionDocumentId, SelectionSourceId};
use editor_aqua::ui::markdown::MarkdownEngine;
use editor_aqua::ui::markdown::presenter::MarkdownPresenter;
use editor_aqua::ui::markdown::visual::BodyTone;
use editor_aqua::ui::theme::{ArtisanTheme, ThemeMode};
use std::rc::Rc;

#[test]
fn completed_image_invalidates_retained_loading_row() {
    use aqua::images::{ImageAssets, ImageStatus};
    use aqua_ui::{
        collection::List,
        style::{Length, Style},
        tree::NodeKind,
    };
    use base64::Engine as _;
    use editor_aqua::ui::{
        markdown::presenter::MediaServices,
        media::{
            image::Images,
            source::{MediaFetcher, MediaSource},
        },
    };
    use std::{
        cell::Cell,
        sync::Arc,
        time::{Duration, Instant},
    };

    let mut encoded = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(2, 1)
        .write_to(&mut encoded, image::ImageFormat::Png)
        .unwrap();
    let source: MediaSource = format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(encoded.into_inner())
    )
    .into();
    let fetcher = MediaFetcher::new("retained-image", |_| {
        Box::pin(async { Err(editor_aqua::ui::media::source::MediaFailure::Missing) })
    })
    .owned_by(&[1], 1);
    let images = Images::new();
    let assets = ImageAssets::default();
    let mut presenter = MarkdownPresenter::default();
    presenter
        .prepare(
            &MarkdownEngine::new().unwrap(),
            &format!(
                "```artisan-image\n{}\n```",
                serde_json::json!({"src": source.raw, "width": 2, "height": 1})
            ),
        )
        .unwrap();
    presenter
        .set_media_services(
            "item",
            MediaServices {
                width: 320.0,
                fetcher: fetcher.clone(),
                images: images.clone(),
                player: editor_audio::Player::new(),
                audio_assets: editor_aqua::ui::media::audio::AudioAssets::new(),
                model_assets: editor_aqua::ui::media::model::load::ModelAssets::new(),
                image_assets: assets.clone(),
                model_capability: Rc::new(|| None),
                scale: 1.0,
                effect: Rc::new(|_| {}),
                wake: Arc::new(|| {}),
            },
        )
        .unwrap();
    let presenter = Rc::new(presenter);
    let settled = Rc::new(Cell::new(false));
    let renders = Rc::new(Cell::new(0));
    let list = || {
        let revision = settled.clone();
        let settled = settled.clone();
        let presenter = presenter.clone();
        let renders = renders.clone();
        List::new()
            .items([0_usize])
            .key(|_| "image-row")
            .revision(move |_| revision.get().then_some(1))
            .row(move |_| {
                renders.set(renders.get() + 1);
                let invalidated = settled.clone();
                let element = presenter
                    .render(
                        "item",
                        ArtisanTheme::for_mode(ThemeMode::Dark),
                        BodyTone::Foreground,
                        SelectionDocumentId(1),
                        &[1],
                        |index| SelectionSourceId(index as u64 + 1),
                        Rc::new(|_, _, _| aqua_ui::element::Box::new().build()),
                        Rc::new(move || invalidated.set(false)),
                    )
                    .unwrap()
                    .element;
                settled.set(true);
                element
            })
            .style(
                Style::new()
                    .width(Length::Px(400.0))
                    .height(Length::Px(300.0)),
            )
            .build()
    };
    let loading = |app: &aqua::UiApp| {
        app.tree()
            .traverse()
            .into_iter()
            .any(|node| match &app.tree().get(node).unwrap().kind {
                NodeKind::Text(text) => text.text().contains("Loading image"),
                _ => false,
            })
    };
    let mut app = aqua::UiApp::new(aqua_text::SharedFonts::new(), aqua_text::SharedRuns::new());
    app.mount(|_| list()).unwrap();
    app.set_viewport(400.0, 300.0, 1.0);
    app.layout().unwrap();
    assert!(
        loading(&app),
        "the pending image must first mount its loading state"
    );
    app.refresh(vec![list()]).unwrap();
    app.layout().unwrap();
    assert_eq!(renders.get(), 1, "unchanged settled rows must be retained");

    let deadline = Instant::now() + Duration::from_secs(5);
    let ready = loop {
        images.drain(&assets);
        presenter.drain_media();
        if let Some(result) = images.request(&fetcher, &source, "image-0", 99, Arc::new(|| {})) {
            break result.unwrap();
        }
        assert!(
            Instant::now() < deadline,
            "the synthetic PNG must finish decoding"
        );
        std::thread::sleep(Duration::from_millis(2));
    };
    assert!(
        matches!(
            ready.status(),
            ImageStatus::Ready {
                width: 2,
                height: 1
            }
        ),
        "the completion must contain decoded image pixels"
    );
    app.refresh(vec![list()]).unwrap();
    app.layout().unwrap();
    assert_eq!(
        renders.get(),
        2,
        "media completion must invalidate the retained row"
    );
    assert!(
        !loading(&app),
        "the rebuilt row must replace its loading state"
    );
    assert!(
        app.tree()
            .traverse()
            .into_iter()
            .any(|node| match &app.tree().get(node).unwrap().kind {
                NodeKind::Media(media) => media.image.is_some(),
                _ => false,
            }),
        "the rebuilt row must mount the Ready image"
    );
    assert!(
        !presenter.drain_media(),
        "an unchanged Ready image must not invalidate again"
    );
    assert!(settled.get(), "the rebuilt image row can settle again");
}

#[test]
fn preparation_proof_and_complete_selection_follow_source_revision() {
    let engine = MarkdownEngine::new().unwrap();
    let mut presenter = MarkdownPresenter::default();
    let original = presenter
        .prepare(&engine, "## Heading\n\nA **strong** paragraph.")
        .unwrap();
    let rendered = presenter
        .render(
            "item",
            ArtisanTheme::for_mode(ThemeMode::Dark),
            BodyTone::Foreground,
            SelectionDocumentId(1),
            &[7],
            |index| SelectionSourceId(index as u64 + 10),
            Rc::new(|_, _, _| aqua_ui::element::ui::Box::new().build()),
            Rc::new(|| {}),
        )
        .unwrap();
    assert_eq!(rendered.selection_sources.len(), 2);
    assert_eq!(&*rendered.selection_sources[0].text, "Heading");
    assert_eq!(&*rendered.selection_sources[1].text, "A strong paragraph.");
    assert!(presenter.is_current(&original));
    assert!(presenter.is_current(&rendered.proof));
    presenter.prepare(&engine, "Different text").unwrap();
    assert!(!presenter.is_current(&original));
    assert!(!presenter.is_current(&rendered.proof));
}

#[test]
fn nested_lists_keep_selection_leaves_in_source_order() {
    let engine = MarkdownEngine::new().unwrap();
    let mut presenter = MarkdownPresenter::default();
    presenter
        .prepare(&engine, "Before\n\n- alpha\n  - beta\n- gamma\n\nAfter")
        .unwrap();
    let rendered = presenter
        .render(
            "item",
            ArtisanTheme::for_mode(ThemeMode::Dark),
            BodyTone::Foreground,
            SelectionDocumentId(1),
            &[7],
            |index| SelectionSourceId(index as u64 + 10),
            Rc::new(|_, _, _| aqua_ui::element::ui::Box::new().build()),
            Rc::new(|| {}),
        )
        .unwrap();
    let values: Vec<_> = rendered
        .selection_sources
        .iter()
        .map(|source| source.text.as_ref())
        .collect();
    assert_eq!(values, ["Before", "alpha", "beta", "gamma", "After"]);
    assert_eq!(rendered.selection_sources[1].separator.as_ref(), "\n");
    assert!(
        rendered
            .selection_sources
            .windows(2)
            .all(|pair| pair[0].order < pair[1].order)
    );
}

#[test]
fn table_cells_register_tab_and_row_separators_in_order() {
    let engine = MarkdownEngine::new().unwrap();
    let mut presenter = MarkdownPresenter::default();
    presenter
        .prepare(&engine, "| A | B |\n|---|---|\n| one | two |")
        .unwrap();
    let rendered = presenter
        .render(
            "item",
            ArtisanTheme::for_mode(ThemeMode::Dark),
            BodyTone::Foreground,
            SelectionDocumentId(1),
            &[7],
            |index| SelectionSourceId(index as u64 + 10),
            Rc::new(|_, _, _| aqua_ui::element::ui::Box::new().build()),
            Rc::new(|| {}),
        )
        .unwrap();
    let values: Vec<_> = rendered
        .selection_sources
        .iter()
        .map(|source| (source.text.as_ref(), source.separator.as_ref()))
        .collect();
    assert_eq!(
        values,
        [("A", "\n"), ("B", "\t"), ("one", "\n"), ("two", "\t")]
    );
    assert!(
        rendered
            .selection_sources
            .windows(2)
            .all(|pair| pair[0].order < pair[1].order)
    );
}

#[test]
fn projected_rows_share_complete_sources_and_revision_bound_ranges() {
    let engine = MarkdownEngine::new().unwrap();
    let source = "Before\n\n- alpha\n  - beta\n\n| A | B |\n|---|---|\n| one | two |";
    let mut presenter = MarkdownPresenter::default();
    presenter.prepare(&engine, source).unwrap();
    assert_eq!(presenter.projected_count(), Some(5));
    let allocations = std::cell::Cell::new(0);
    let render = |row| {
        presenter
            .render_projected(
                row,
                ArtisanTheme::for_mode(ThemeMode::Dark),
                BodyTone::Foreground,
                SelectionDocumentId(1),
                &[7],
                |index| {
                    allocations.set(allocations.get() + 1);
                    SelectionSourceId(index as u64 + 10)
                },
                None,
                None,
                None,
            )
            .unwrap()
    };
    let info = presenter.projected_leaf_info(2).unwrap();
    assert_eq!(info.list_depth, 2);
    assert!(presenter.projected_leaf_is_current(2, &info));
    let first = render(0);
    let initial_allocations = allocations.get();
    let nested = render(2);
    let table = render(4);
    assert_eq!(allocations.get(), initial_allocations);
    assert!(std::sync::Arc::ptr_eq(
        &first.selection_batch,
        &nested.selection_batch
    ));
    assert!(std::sync::Arc::ptr_eq(
        &first.selection_batch,
        &table.selection_batch
    ));
    assert!(std::sync::Arc::ptr_eq(
        &first.selection_sources,
        &nested.selection_sources
    ));
    assert!(std::sync::Arc::ptr_eq(
        &first.selection_sources,
        &table.selection_sources
    ));
    let selected = |view: &editor_aqua::ui::markdown::presenter::ProjectedMarkdown| {
        view.selection_sources
            .iter()
            .map(|source| source.text.to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        selected(&first),
        ["Before", "alpha", "beta", "A", "B", "one", "two"]
    );
    assert_eq!(selected(&nested), selected(&first));
    assert_eq!(selected(&table), selected(&first));
    assert!(source[nested.source_range.clone()].contains("beta"));
    assert!(presenter.projected_is_current(2, &nested.source_range, &nested.proof));
    presenter.prepare(&engine, "replacement").unwrap();
    assert!(!presenter.projected_is_current(2, &nested.source_range, &nested.proof));
    assert!(!presenter.projected_leaf_is_current(2, &info));
    let replacement = presenter
        .render_projected(
            0,
            ArtisanTheme::for_mode(ThemeMode::Dark),
            BodyTone::Foreground,
            SelectionDocumentId(1),
            &[7],
            |index| SelectionSourceId(index as u64 + 10),
            None,
            None,
            None,
        )
        .unwrap();
    assert_eq!(
        first.selection_batch.owner(),
        replacement.selection_batch.owner()
    );
    assert!(first.selection_batch.revision() < replacement.selection_batch.revision());
    assert!(!std::sync::Arc::ptr_eq(
        &first.selection_batch,
        &replacement.selection_batch
    ));
}

#[test]
fn projected_ready_rich_row_uses_retained_dispatch_and_complete_text_sources() {
    let engine = MarkdownEngine::new().unwrap();
    let mut presenter = MarkdownPresenter::default();
    let source = "Before\n\n```artisan-heatmap\n{\"values\":[[1]]}\n```\n\nAfter";
    presenter.prepare(&engine, source).unwrap();
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    let services = editor_aqua::ui::markdown::presenter::HeatmapServices::new(*theme);
    let row = presenter
        .render_projected_with_services(
            1,
            "item",
            theme,
            BodyTone::Foreground,
            SelectionDocumentId(1),
            &[7],
            |index| SelectionSourceId(index as u64 + 10),
            Rc::new(|_, _, _| aqua_ui::element::ui::Box::new().build()),
            Rc::new(|| {}),
            None,
            None,
            None,
            Some(&services),
        )
        .unwrap();
    let values: Vec<_> = row
        .selection_sources
        .iter()
        .map(|source| source.text.as_ref())
        .collect();
    assert_eq!(values, ["Before", "After"]);
    assert!(source[row.source_range.clone()].contains("artisan-heatmap"));
    assert!(presenter.projected_is_current(1, &row.source_range, &row.proof));
}

#[test]
fn projected_code_keeps_offscreen_line_sources_and_display_copy() {
    let engine = MarkdownEngine::new().unwrap();
    let mut presenter = MarkdownPresenter::default();
    let source = "Before\n\n```rust\nlet x = 1;\nlet y = 2;\n```\n\nAfter";
    presenter.prepare(&engine, source).unwrap();
    let captured = Rc::new(std::cell::RefCell::new(String::new()));
    let capture = captured.clone();
    let code = editor_aqua::ui::markdown::presenter::CodeServices {
        copy_enabled: true,
        reference: None,
        copy: Rc::new(move |_, text| {
            *capture.borrow_mut() = text.to_owned();
            aqua_ui::element::ui::Box::new().build()
        }),
    };
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    let row = presenter
        .render_projected_with_code(
            1,
            "item",
            theme,
            BodyTone::Foreground,
            SelectionDocumentId(1),
            &[7],
            |index| SelectionSourceId(index as u64 + 10),
            Rc::new(|_, _, _| aqua_ui::element::ui::Box::new().build()),
            Rc::new(|| {}),
            None,
            None,
            None,
            None,
            Some(&code),
        )
        .unwrap();
    let values: Vec<_> = row
        .selection_sources
        .iter()
        .map(|source| source.text.as_ref())
        .collect();
    assert_eq!(values, ["Before", "let x = 1;", "let y = 2;", "After"]);
    assert_eq!(captured.borrow().as_str(), "let x = 1;\nlet y = 2;");
}

#[cfg(feature = "test-support")]
#[test]
fn rich_identity_survives_surrounding_edits_but_not_replacement() {
    let engine = MarkdownEngine::new().unwrap();
    let mut presenter = MarkdownPresenter::default();
    let chart = "```artisan-heatmap\n{\"values\":[[1]]}\n```";
    let first = presenter
        .prepare(&engine, &format!("before\n\n{chart}"))
        .unwrap();
    let id = presenter.rich_identities()[0];
    presenter
        .prepare(&engine, &format!("extra\n\nbefore\n\n{chart}\n\nafter"))
        .unwrap();
    assert_eq!(presenter.rich_identities(), [id]);
    assert!(!presenter.is_current(&first));
    presenter.prepare(&engine, "plain only").unwrap();
    assert!(presenter.rich_identities().is_empty());
    presenter.prepare(&engine, chart).unwrap();
    assert_ne!(presenter.rich_identities(), [id]);
}

#[test]
fn projected_reveal_metadata_tracks_keyed_cells_and_streaming_tail() {
    let engine = MarkdownEngine::new().unwrap();
    let mut presenter = MarkdownPresenter::default();
    presenter
        .prepare(
            &engine,
            "Before\n\n| A | B |\n|---|---|\n| one | two words |",
        )
        .unwrap();
    let render = |row| {
        presenter
            .render_projected(
                row,
                ArtisanTheme::for_mode(ThemeMode::Dark),
                BodyTone::Foreground,
                SelectionDocumentId(9),
                &[3],
                |index| SelectionSourceId(index as u64 + 100),
                None,
                None,
                None,
            )
            .unwrap()
    };
    let row = render(2);
    let same = render(2);
    assert_eq!(row.geometry.frame_key, same.geometry.frame_key);
    assert_eq!(row.geometry.text.len(), 2);
    assert_ne!(row.geometry.text[0].key, row.geometry.text[1].key);
    assert_eq!(row.geometry.tail, Some(row.geometry.text[1].slot));
    let reveal = editor_aqua::ui::markdown::reveal::PreparedReveal::default();
    reveal.begin_frame(std::time::Instant::now(), true, true, false, false);
    let text = row.reveal_text(&reveal);
    assert_eq!(text[0].end, text[0].text.len());
    assert_eq!(&text[1].text[..text[1].end], "two words");
    assert!(std::sync::Arc::ptr_eq(
        &row.selection_batch,
        &same.selection_batch
    ));
}

#[test]
fn mermaid_fence_renders_as_a_diagram_with_source_copy_and_no_text_selection() {
    let engine = MarkdownEngine::new().unwrap();
    let mut presenter = MarkdownPresenter::default();
    let diagram = "flowchart LR\n  A[Start] --> B[End]\n";
    let source = format!(
        "Before\n\n```mermaid\n{diagram}```\n\n```mermaid\nnot a diagram {{{{\n```\n\nAfter"
    );
    presenter.prepare(&engine, &source).unwrap();
    let copied = Rc::new(std::cell::RefCell::new(Vec::new()));
    let capture = copied.clone();
    let code = editor_aqua::ui::markdown::presenter::CodeServices {
        copy_enabled: true,
        reference: None,
        copy: Rc::new(move |_, text| {
            capture.borrow_mut().push(text.to_owned());
            aqua_ui::element::ui::Box::new().build()
        }),
    };
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    let render = |row: usize| {
        presenter
            .render_projected_with_code(
                row,
                "item",
                theme,
                BodyTone::Foreground,
                SelectionDocumentId(1),
                &[7],
                |index| SelectionSourceId(index as u64 + 10),
                Rc::new(|_, _, _| aqua_ui::element::ui::Box::new().build()),
                Rc::new(|| {}),
                None,
                None,
                None,
                None,
                Some(&code),
            )
            .unwrap()
    };
    let drawn = render(1);
    let texts: Vec<_> = drawn
        .selection_sources
        .iter()
        .map(|source| source.text.as_ref())
        .collect();
    assert_eq!(texts, ["Before", "not a diagram {{", "After"]);
    assert_eq!(
        copied.borrow().as_slice(),
        ["flowchart LR\n  A[Start] --> B[End]"]
    );
    render(2);
    assert_eq!(copied.borrow().len(), 2);
}

#[test]
fn streaming_rich_fence_renders_its_placeholder_and_invalid_one_keeps_its_code() {
    let engine = MarkdownEngine::new().unwrap();
    let mut presenter = MarkdownPresenter::default();
    let source = "Before\n\n```artisan-stats\n{\"nope\":1}\n```\n\nMid\n\n```artisan-image-compare\n{\"before\":";
    presenter.prepare(&engine, source).unwrap();
    let code = editor_aqua::ui::markdown::presenter::CodeServices {
        copy_enabled: false,
        reference: None,
        copy: Rc::new(|_, _| aqua_ui::element::ui::Box::new().build()),
    };
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    for row in 0..presenter.projected_count().unwrap() {
        presenter
            .render_projected_with_code(
                row,
                "item",
                theme,
                BodyTone::Foreground,
                SelectionDocumentId(1),
                &[7],
                |index| SelectionSourceId(index as u64 + 10),
                Rc::new(|_, _, _| aqua_ui::element::ui::Box::new().build()),
                Rc::new(|| {}),
                None,
                None,
                None,
                None,
                Some(&code),
            )
            .unwrap();
    }
}

#[test]
fn leaf_proofs_survive_appends_but_not_edits_to_their_own_leaf() {
    let engine = MarkdownEngine::new().unwrap();
    let mut presenter = MarkdownPresenter::default();
    presenter
        .prepare(&engine, "alpha\n\nbeta\n\ngam")
        .unwrap();
    let first = presenter.projected_leaf_info(0).unwrap();
    let second = presenter.projected_leaf_info(1).unwrap();
    let third = presenter.projected_leaf_info(2).unwrap();
    presenter
        .prepare(&engine, "alpha\n\nbeta\n\ngamma\n\ndelta")
        .unwrap();
    assert_eq!(presenter.projected_count(), Some(4));
    assert!(presenter.projected_leaf_is_current(0, &first));
    assert!(presenter.projected_leaf_is_current(1, &second));
    assert!(!presenter.projected_leaf_is_current(2, &third));
    presenter
        .prepare(&engine, "alpha\n\nbetter\n\ngamma\n\ndelta")
        .unwrap();
    assert!(presenter.projected_leaf_is_current(0, &first));
    assert!(!presenter.projected_leaf_is_current(1, &second));
}
