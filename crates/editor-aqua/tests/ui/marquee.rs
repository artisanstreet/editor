use aqua::UiApp;
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::{
    element::{Box as UiBox, Key},
    tree::hash_key,
};
use editor_aqua::ui::marquee::Marquee;

#[test]
fn overflow_waits_then_scrolls_and_reduced_motion_stops() {
    let marquee = Marquee::new("note", "A long note that should not fit in a small space");
    let fonts = SharedFonts::new();
    editor_aqua::ui::fonts::register_bundled_fonts(&fonts);
    let mut app = UiApp::new(fonts, SharedRuns::new());
    app.mount(|_| {
        UiBox::new()
            .width(60.0)
            .height(20.0)
            .child(marquee.build(20.0))
            .build()
    })
    .unwrap();
    app.set_viewport(60.0, 20.0, 1.0);
    app.build_frame().unwrap();
    let mut marquee = marquee;
    let owner_key = hash_key(Key::from("note"));
    let owner = app
        .tree()
        .traverse()
        .into_iter()
        .find(|node| {
            app.tree()
                .get(*node)
                .is_some_and(|record| record.key == Some(owner_key))
        })
        .unwrap();
    assert!(marquee.advance(&mut app, owner, 0.0, false));
    let key = hash_key(Key::from("note-first"));
    let first = app
        .tree()
        .traverse()
        .into_iter()
        .find(|node| {
            app.tree()
                .get(*node)
                .is_some_and(|record| record.key == Some(key))
        })
        .unwrap();
    assert!(marquee.advance(&mut app, owner, 1.6, false));
    assert_eq!(
        app.tree().get(first).unwrap().paint().transform.unwrap().tx,
        0.0
    );
    assert!(marquee.advance(&mut app, owner, 2.6, false));
    assert!((app.tree().get(first).unwrap().paint().transform.unwrap().tx + 36.0).abs() < 0.001);
    assert!(!marquee.advance(&mut app, owner, 2.7, true));
    assert_eq!(
        app.tree().get(first).unwrap().paint().transform.unwrap().tx,
        0.0
    );
}
