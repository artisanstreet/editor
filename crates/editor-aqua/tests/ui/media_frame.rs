use aqua::UiApp;
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::{
    element::{Box as UiBox, Key},
    style::{Axis, Style},
    tree::hash_key,
};
use editor_aqua::ui::{media_frame::MediaFrame, theme::ArtisanTheme};

fn frame_bounds(app: &UiApp, key: &str) -> aqua_core::Rect {
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

fn frames(width: f32) -> aqua_ui::element::Element {
    let theme = ArtisanTheme::resolved();
    UiBox::new()
        .width(700.0)
        .height(800.0)
        .style(Style::new().axis(Axis::Column))
        .child(
            MediaFrame::new(theme, 2.0, width, UiBox::new().build())
                .build()
                .with_key("aspect"),
        )
        .child(
            MediaFrame::sent_attachment(theme, UiBox::new().build())
                .build()
                .with_key("sent"),
        )
        .child(
            MediaFrame::transcript_image(theme, 1.0, width, UiBox::new().build())
                .build()
                .with_key("transcript"),
        )
        .build()
}

#[test]
fn measured_width_changes_generic_and_transcript_frames_but_not_sent_thumbnail() {
    let mut app = UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| frames(640.0)).unwrap();
    app.set_viewport(700.0, 800.0, 1.0);
    app.build_frame().unwrap();
    let aspect = frame_bounds(&app, "aspect");
    let sent = frame_bounds(&app, "sent");
    let transcript = frame_bounds(&app, "transcript");
    assert_eq!((aspect.w, aspect.h), (640.0, 320.0));
    assert_eq!((sent.w, sent.h), (96.0, 96.0));
    assert_eq!((transcript.w, transcript.h), (576.0, 324.0));
    app.refresh(vec![frames(320.0)]).unwrap();
    app.build_frame().unwrap();
    let aspect = frame_bounds(&app, "aspect");
    let sent = frame_bounds(&app, "sent");
    let transcript = frame_bounds(&app, "transcript");
    assert_eq!((aspect.w, aspect.h), (320.0, 160.0));
    assert_eq!((sent.w, sent.h), (96.0, 96.0));
    assert_eq!((transcript.w, transcript.h), (320.0, 180.0));
}

#[test]
fn visibility_waits_for_paint_and_actual_clip_intersection() {
    use std::{cell::Cell, rc::Rc};
    let count = Rc::new(Cell::new(0));
    let frame = |top: f32| {
        let count = count.clone();
        UiBox::new()
            .width(100.0)
            .height(100.0)
            .clip(true)
            .child(
                MediaFrame::new(ArtisanTheme::resolved(), 1.0, 60.0, UiBox::new().build())
                    .on_visible(move || count.set(count.get() + 1))
                    .build()
                    .style(Style::new().absolute_at(Some(0.0), Some(top), None, None)),
            )
            .build()
    };
    let mut app = UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| frame(100.0)).unwrap();
    app.set_viewport(100.0, 100.0, 1.0);
    app.layout().unwrap();
    assert_eq!(count.get(), 0);
    app.build_frame().unwrap();
    assert_eq!(count.get(), 0);
    app.refresh(vec![frame(90.0)]).unwrap();
    app.layout().unwrap();
    assert_eq!(count.get(), 0);
    app.build_frame().unwrap();
    assert_eq!(count.get(), 1);
    app.build_frame().unwrap();
    assert_eq!(count.get(), 1);
    app.refresh(vec![frame(120.0)]).unwrap();
    app.build_frame().unwrap();
    assert_eq!(count.get(), 1);
}

#[test]
fn visible_probe_does_not_intercept_nested_retry_activation() {
    use aqua_core::{InputEvent, Point, PointerButton, PointerEvent, PointerKind};
    use aqua_ui::element::Button;
    use std::{cell::Cell, rc::Rc};

    let retries = Rc::new(Cell::new(0));
    let visible = Rc::new(Cell::new(0));
    let retry_count = retries.clone();
    let visible_count = visible.clone();
    let mut app = UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| {
        let retry = Button::new()
            .width(50.0)
            .height(30.0)
            .on_activate(move || retry_count.set(retry_count.get() + 1))
            .build();
        UiBox::new()
            .width(100.0)
            .height(100.0)
            .child(
                MediaFrame::new(ArtisanTheme::resolved(), 1.0, 100.0, retry)
                    .on_visible(move || visible_count.set(visible_count.get() + 1))
                    .build(),
            )
            .build()
    })
    .unwrap();
    app.set_viewport(100.0, 100.0, 1.0);
    app.build_frame().unwrap();
    assert_eq!(visible.get(), 1);
    let pointer = PointerEvent {
        contact: 0,
        kind: PointerKind::Mouse,
        position: Point::new(10.0, 10.0),
        buttons: 1,
    };
    app.handle_input(&InputEvent::PointerDown(pointer, PointerButton::Primary));
    app.handle_input(&InputEvent::PointerUp(
        PointerEvent {
            buttons: 0,
            ..pointer
        },
        PointerButton::Primary,
    ));
    assert_eq!(retries.get(), 1);
}
