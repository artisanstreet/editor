use aqua::UiApp;
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::{
    element::{Box as UiBox, Key},
    tree::hash_key,
};
use editor_aqua::ui::{hover::HoverTone, picker_motion::HoverSurface, theme::ArtisanTheme};

#[test]
fn hover_pill_tracks_measured_rows_and_cancels_motion_on_dispose() {
    let theme = ArtisanTheme::resolved();
    let mut surface = HoverSurface::new("hover-pill");
    let pill = surface.pill(&theme, 14.0, true);
    let mut app = UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| {
        UiBox::new()
            .width(100.0)
            .height(80.0)
            .layout("column")
            .child(pill)
            .child(
                UiBox::new()
                    .width(100.0)
                    .height(20.0)
                    .build()
                    .with_key("first"),
            )
            .child(
                UiBox::new()
                    .width(100.0)
                    .height(20.0)
                    .build()
                    .with_key("second"),
            )
            .build()
    })
    .unwrap();
    app.set_viewport(100.0, 80.0, 1.0);
    app.build_frame().unwrap();
    let root = app.tree().root().unwrap();
    let pill_key = hash_key(Key::from("hover-pill"));
    let pill = app
        .tree()
        .traverse()
        .into_iter()
        .find(|node| {
            app.tree()
                .get(*node)
                .is_some_and(|record| record.key == Some(pill_key))
        })
        .unwrap();
    surface.hovered("first", true, HoverTone::Neutral);
    let initial = surface
        .advance_frame(&mut app, root, &theme, true, 0.0, false)
        .unwrap();
    assert!(initial.changed);
    assert!(!initial.animating);
    let unchanged = surface
        .advance_frame(&mut app, root, &theme, true, 0.0, false)
        .unwrap();
    assert!(!unchanged.changed);
    assert!(!unchanged.animating);
    app.build_frame().unwrap();
    let start = app.tree().visual_rect(pill).unwrap().y;
    surface.hovered("second", true, HoverTone::Neutral);
    assert!(
        surface
            .advance(&mut app, root, &theme, true, 0.01, false)
            .unwrap()
    );
    assert!(
        surface
            .advance(&mut app, root, &theme, true, 0.1, false)
            .unwrap()
    );
    app.build_frame().unwrap();
    let midway = app.tree().visual_rect(pill).unwrap().y;
    assert!(midway > start && midway < start + 20.0);
    let settled = surface
        .advance_frame(&mut app, root, &theme, true, 0.3, false)
        .unwrap();
    assert!(settled.changed);
    assert!(!settled.animating);
    app.build_frame().unwrap();
    assert!((app.tree().visual_rect(pill).unwrap().y - start - 20.0).abs() < 0.01);
    surface.hovered("first", true, HoverTone::Neutral);
    assert!(
        surface
            .advance(&mut app, root, &theme, true, 0.4, false)
            .unwrap()
    );
    surface.dispose(&mut app);
    assert!(app.host().motion().is_empty());
}

#[test]
fn reduced_motion_reports_immediate_geometry_change() {
    let theme = ArtisanTheme::resolved();
    let mut surface = HoverSurface::new("reduced-pill");
    let pill = surface.pill(&theme, 14.0, true);
    let mut app = UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| {
        UiBox::new()
            .width(100.0)
            .height(80.0)
            .layout("column")
            .child(pill)
            .child(
                UiBox::new()
                    .width(100.0)
                    .height(20.0)
                    .build()
                    .with_key("first"),
            )
            .child(
                UiBox::new()
                    .width(100.0)
                    .height(20.0)
                    .build()
                    .with_key("second"),
            )
            .build()
    })
    .unwrap();
    app.set_viewport(100.0, 80.0, 1.0);
    app.build_frame().unwrap();
    let root = app.tree().root().unwrap();
    surface.hovered("first", true, HoverTone::Neutral);
    surface
        .advance_frame(&mut app, root, &theme, true, 0.0, true)
        .unwrap();
    app.build_frame().unwrap();
    surface.hovered("second", true, HoverTone::Neutral);
    let frame = surface
        .advance_frame(&mut app, root, &theme, true, 0.1, true)
        .unwrap();
    assert!(frame.changed);
    assert!(!frame.animating);
    let next = surface
        .advance_frame(&mut app, root, &theme, true, 0.1, true)
        .unwrap();
    assert!(!next.changed);
    assert!(!next.animating);
    assert!(app.host().motion().is_empty());
}
