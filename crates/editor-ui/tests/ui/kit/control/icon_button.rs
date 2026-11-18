use aqua_core::{InputEvent, Point, PointerButton, PointerEvent, PointerKind};
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::element::{Box as UiBox, Key};
use aqua_ui::tree::hash_key;
use editor_assets::AssetId;
use editor_ui::kit::{Finish, IconButton, IconButtonSize};
use editor_ui::theme::{ArtisanTheme, Motion};

fn pressed_shift(motion: Motion) -> Option<aqua_core::Transform2D> {
    let mut theme = ArtisanTheme::resolved();
    theme.interaction.motion = motion;
    let button = IconButton::new(
        "close",
        theme,
        AssetId::TABLER_X,
        "Close",
        IconButtonSize::Sm,
        Finish::Glass,
    )
    .on_activate(|_| {})
    .build();
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| UiBox::new().padding(20.0).child(button).build())
        .unwrap();
    app.set_viewport(200.0, 200.0, 1.0);
    app.build_frame().unwrap();
    let node = app
        .tree()
        .sole_node_with_key(hash_key(Key::from("close")))
        .unwrap();
    let rest = app.tree().visual_rect(node).unwrap();
    let pointer = PointerEvent {
        contact: 0,
        kind: PointerKind::Mouse,
        position: Point::new(rest.x + rest.w / 2.0, rest.y + rest.h / 2.0),
        buttons: 1,
    };
    app.handle_input(&InputEvent::PointerDown(pointer, PointerButton::Primary));
    app.build_frame().unwrap();
    app.tree()
        .get(node)
        .and_then(|record| record.paint().transform)
        .filter(|shift| *shift != aqua_core::Transform2D::IDENTITY)
}

#[test]
fn pressing_a_button_holds_it_still_under_reduced_motion() {
    assert!(pressed_shift(Motion::Full).is_some());
    assert_eq!(pressed_shift(Motion::Reduced), None);
}
