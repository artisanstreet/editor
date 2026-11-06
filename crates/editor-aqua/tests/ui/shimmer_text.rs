use aqua::UiApp;
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::element::{Box as UiBox, Text};
use editor_aqua::ui::{
    shimmer_text::{ShimmerLine, ShimmerTiming},
    theme::ArtisanTheme,
};

#[test]
fn retained_shimmer_updates_only_when_configuration_or_bounds_change() {
    let mut shimmer =
        ShimmerLine::new("status", &ArtisanTheme::resolved()).timing(ShimmerTiming::Status);
    let mut app = UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| {
        UiBox::new()
            .width(200.0)
            .height(30.0)
            .child(shimmer.build(Text::new("Working").build().with_key("status")))
            .build()
    })
    .unwrap();
    app.set_viewport(200.0, 30.0, 1.0);
    app.build_frame().unwrap();
    let key = aqua_ui::tree::hash_key(aqua_ui::element::Key::from("status"));
    let node = app
        .tree()
        .traverse()
        .into_iter()
        .find(|node| {
            app.tree()
                .get(*node)
                .is_some_and(|record| record.key == Some(key))
        })
        .unwrap();
    assert!(shimmer.motion.sync(&mut app, node, false));
    let retained = app.tree().paint(node).unwrap().gpui_text_shimmer.unwrap();
    assert_eq!(
        retained.map.bounds,
        app.tree().pre_transform_rect(node).unwrap()
    );
    assert_eq!(retained.duration, std::time::Duration::from_secs(3));
    assert_eq!(retained.delay, std::time::Duration::from_millis(1_500));
    assert!(!shimmer.motion.sync(&mut app, node, false));
    assert!(shimmer.motion.sync(&mut app, node, true));
    assert!(app.tree().paint(node).unwrap().gpui_text_shimmer.is_none());
    assert!(shimmer.motion.sync(&mut app, node, false));
    assert_eq!(
        app.tree()
            .paint(node)
            .unwrap()
            .gpui_text_shimmer
            .unwrap()
            .key,
        retained.key
    );
    shimmer.motion.set_active(false);
    assert!(shimmer.motion.sync(&mut app, node, false));
    assert!(app.tree().paint(node).unwrap().gpui_text_shimmer.is_none());
}
