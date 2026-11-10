use aqua::UiApp;
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::{
    element::{Box as UiBox, Key},
    tree::{NodeKind, hash_key},
};
use editor_aqua::{
    conversation::{
        compaction::{CompactionFrame, CompactionPhase},
        view::compaction,
    },
    ui::theme::ArtisanTheme,
};

fn frame(swap: Option<(CompactionPhase, f32)>) -> CompactionFrame {
    CompactionFrame {
        open: 1.0,
        draw: 1.0,
        label: 1.0,
        swap,
        pulse: None,
    }
}

fn node(app: &UiApp, key: &str) -> aqua_core::NodeId {
    let key = hash_key(Key::from(key));
    app.tree()
        .traverse()
        .into_iter()
        .find(|node| {
            app.tree()
                .get(*node)
                .is_some_and(|record| record.key == Some(key))
        })
        .unwrap()
}

#[test]
fn retained_label_switches_from_running_to_done_and_removes_shimmer() {
    let theme = ArtisanTheme::resolved();
    let fonts = SharedFonts::new();
    editor_aqua::ui::fonts::register_bundled_fonts(&fonts);
    let mut app = UiApp::new(fonts, SharedRuns::new());
    let render = |phase, current| {
        UiBox::new()
            .width(400.0)
            .height(40.0)
            .child(compaction::label(phase, None, current, theme))
            .build()
            .with_key("row")
    };
    app.mount(|_| render(CompactionPhase::Running, frame(None)))
        .unwrap();
    app.set_viewport(400.0, 40.0, 1.0);
    app.build_frame().unwrap();
    let row = node(&app, "row");
    assert!(compaction::sync_label(
        &mut app,
        row,
        CompactionPhase::Running,
        false,
        frame(None),
        &theme,
        false
    ));
    let text = node(&app, "compaction-shimmer");
    assert!(app.tree().paint(text).unwrap().gpui_text_shimmer.is_some());
    assert!(!compaction::sync_label(
        &mut app,
        row,
        CompactionPhase::Running,
        false,
        frame(None),
        &theme,
        false
    ));
    let outgoing = frame(Some((CompactionPhase::Running, 0.25)));
    app.refresh(vec![render(CompactionPhase::Done, outgoing)])
        .unwrap();
    app.build_frame().unwrap();
    let text = node(&app, "compaction-shimmer");
    let NodeKind::Text(content) = &app.tree().get(text).unwrap().kind else {
        panic!("expected text")
    };
    assert_eq!(content.text(), "Compacting");
    compaction::sync_label(
        &mut app,
        row,
        CompactionPhase::Done,
        false,
        outgoing,
        &theme,
        false,
    );
    assert!(app.tree().paint(text).unwrap().gpui_text_shimmer.is_some());
    let incoming = frame(Some((CompactionPhase::Running, 0.75)));
    app.refresh(vec![render(CompactionPhase::Done, incoming)])
        .unwrap();
    app.build_frame().unwrap();
    let text = node(&app, "compaction-shimmer");
    let NodeKind::Text(content) = &app.tree().get(text).unwrap().kind else {
        panic!("expected text")
    };
    assert_eq!(content.text(), "Compacted");
    compaction::sync_label(
        &mut app,
        row,
        CompactionPhase::Done,
        false,
        incoming,
        &theme,
        false,
    );
    assert!(app.tree().paint(text).unwrap().gpui_text_shimmer.is_none());
}

#[test]
fn compaction_shimmer_keeps_its_clock_when_the_entrance_subtree_is_replaced() {
    let theme = ArtisanTheme::resolved();
    let fonts = SharedFonts::new();
    editor_aqua::ui::fonts::register_bundled_fonts(&fonts);
    let mut app = UiApp::new(fonts, SharedRuns::new());
    let render = |key: &str| {
        UiBox::new()
            .width(400.0)
            .height(40.0)
            .child(
                compaction::label(CompactionPhase::Running, None, frame(None), theme)
                    .with_key(key.to_owned()),
            )
            .build()
            .with_key("row")
    };
    app.mount(|_| render("entering")).unwrap();
    app.set_viewport(400.0, 40.0, 1.0);
    app.build_frame().unwrap();
    let row = node(&app, "row");
    compaction::sync_label(
        &mut app,
        row,
        CompactionPhase::Running,
        false,
        frame(None),
        &theme,
        false,
    );
    let first_node = node(&app, "compaction-shimmer");
    let first = app
        .tree()
        .paint(first_node)
        .unwrap()
        .gpui_text_shimmer
        .unwrap();
    let mut clocks = aqua::render::text::ShimmerClocks::default();
    clocks.begin_frame();
    assert_eq!(clocks.phase(&first, 0.0), 0.0);
    clocks.end_frame();
    app.refresh(vec![render("settled")]).unwrap();
    app.build_frame().unwrap();
    let row = node(&app, "row");
    compaction::sync_label(
        &mut app,
        row,
        CompactionPhase::Running,
        false,
        frame(None),
        &theme,
        false,
    );
    let next_node = node(&app, "compaction-shimmer");
    assert_ne!(first_node, next_node);
    let next = app
        .tree()
        .paint(next_node)
        .unwrap()
        .gpui_text_shimmer
        .unwrap();
    clocks.begin_frame();
    assert!((clocks.phase(&next, 0.55) - 0.275).abs() < 0.001);
    clocks.end_frame();
    clocks.begin_frame();
    assert!((clocks.phase(&next, 1.0) - 0.5).abs() < 0.001);
    clocks.end_frame();
}
