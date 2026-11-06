use aqua::UiApp;
use aqua_core::Rect;
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::{
    AlignItems,
    element::{Box as UiBox, Key},
    style::{Axis, Length, Style},
    tree::hash_key,
};
use editor_aqua::ui::{
    chat_bubble::{ChatBubbleSegment, SegmentPosition},
    theme::ArtisanTheme,
};

fn bounds(app: &UiApp, key: &str) -> Rect {
    let key = hash_key(Key::from(key));
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
    app.tree().rect(node).unwrap()
}

#[test]
fn segments_join_and_resize_to_the_available_width() {
    let theme = ArtisanTheme::resolved();
    let mut app = UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| {
        let mut root = UiBox::new()
            .width(Length::Pct(100.0))
            .height(240.0)
            .style(Style::new().axis(Axis::Column).align(AlignItems::END));
        for index in 0..3 {
            let content = UiBox::new().height(20.0).build();
            let segment =
                ChatBubbleSegment::new(theme, SegmentPosition::for_index(index, 3), content)
                    .build()
                    .with_key(format!("segment-{index}"));
            root = root.child(segment);
        }
        root.build()
    })
    .unwrap();
    app.set_viewport(720.0, 240.0, 1.0);
    app.build_frame().unwrap();
    let head = bounds(&app, "segment-0");
    let body = bounds(&app, "segment-1");
    let tail = bounds(&app, "segment-2");
    assert_eq!(head.w, 576.0);
    assert_eq!(head.w, body.w);
    assert_eq!(body.w, tail.w);
    assert_eq!(head.bottom(), body.y);
    assert_eq!(body.bottom(), tail.y);
    assert_eq!((head.h, body.h, tail.h), (32.0, 20.0, 32.0));
    app.set_viewport(360.0, 240.0, 1.0);
    app.build_frame().unwrap();
    for key in ["segment-0", "segment-1", "segment-2"] {
        assert_eq!(bounds(&app, key).w, 360.0);
    }
}
