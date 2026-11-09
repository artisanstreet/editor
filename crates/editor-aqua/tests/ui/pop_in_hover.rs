use std::time::{Duration, Instant};

use aqua::UiApp;
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::{
    element::{Box as UiBox, Key},
    tree::hash_key,
};
use editor_aqua::ui::{pop_in_hover::PopInHover, theme::ArtisanTheme};

#[test]
fn row_highlights_pop_independently_and_fade_out_without_sliding() {
    let theme = ArtisanTheme::resolved();
    let mut hover = PopInHover::new("row-hover");
    let mut app = UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| {
        UiBox::new()
            .width(100.0)
            .height(80.0)
            .layout("column")
            .child(
                UiBox::new()
                    .width(100.0)
                    .height(40.0)
                    .child(hover.background("first", &theme, 8.0))
                    .build(),
            )
            .child(
                UiBox::new()
                    .width(100.0)
                    .height(40.0)
                    .child(hover.background("second", &theme, 8.0))
                    .build(),
            )
            .build()
    })
    .unwrap();
    app.set_viewport(100.0, 80.0, 1.0);
    app.build_frame().unwrap();
    let root = app.tree().root().unwrap();
    let find = |key| {
        app.tree()
            .sole_node_with_key(hash_key(Key::from(key)))
            .unwrap()
    };
    let first = find("row-hover-first");
    let second = find("row-hover-second");
    let first_bounds = app.tree().rect(first).unwrap();
    let second_bounds = app.tree().rect(second).unwrap();
    let start = Instant::now();
    hover.hovered_at("first", true, start, false);
    assert!(hover.advance_frame(&mut app, root, start, false).animating);
    let middle = start + Duration::from_millis(75);
    hover.advance_frame(&mut app, root, middle, false);
    let paint = app.tree().paint(first).unwrap();
    assert!(paint.opacity.unwrap() > 0.0 && paint.opacity.unwrap() < 1.0);
    assert!(paint.transform.unwrap().d > 0.4 && paint.transform.unwrap().d < 1.0);
    let settled = start + Duration::from_millis(300);
    assert!(
        !hover
            .advance_frame(&mut app, root, settled, false)
            .animating
    );
    hover.hovered_at("second", true, settled, false);
    hover.advance_frame(&mut app, root, settled + Duration::from_millis(50), false);
    for node in [first, second] {
        let opacity = app.tree().paint(node).unwrap().opacity.unwrap();
        assert!(
            opacity > 0.0 && opacity < 1.0,
            "both highlights must animate independently"
        );
    }
    assert_eq!(app.tree().rect(first).unwrap(), first_bounds);
    assert_eq!(app.tree().rect(second).unwrap(), second_bounds);
    hover.hovered_at("second", false, settled + Duration::from_millis(60), false);
    let frame = hover.advance_frame(&mut app, root, settled + Duration::from_millis(300), false);
    assert!(!frame.animating);
    assert_eq!(app.tree().paint(first).unwrap().opacity, Some(0.0));
    assert_eq!(app.tree().paint(second).unwrap().opacity, Some(0.0));
    hover.hovered_at("first", true, settled + Duration::from_secs(1), true);
    assert!(
        !hover
            .advance_frame(&mut app, root, settled + Duration::from_secs(1), true)
            .animating
    );
    assert_eq!(app.tree().paint(first).unwrap().opacity, Some(1.0));
}
