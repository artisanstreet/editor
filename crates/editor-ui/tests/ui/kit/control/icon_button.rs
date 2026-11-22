use aqua_core::{InputEvent, Point, PointerButton, PointerEvent, PointerKind};
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::element::{Box as UiBox, Key};
use aqua_ui::tree::hash_key;
use editor_assets::AssetId;
use editor_ui::kit::{Finish, IconButton, IconButtonSize};
use editor_ui::theme::{ArtisanTheme, Motion};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Press {
    Moves,
    HoldsStill,
}

pub(crate) fn press(
    motion: Motion,
    key: &str,
    control: impl FnOnce(&ArtisanTheme) -> aqua_ui::element::Element,
) -> Option<Press> {
    let mut theme = ArtisanTheme::resolved();
    theme.interaction.motion = motion;
    let button = control(&theme);
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| UiBox::new().child(button).build()).ok()?;
    app.set_viewport(200.0, 200.0, 1.0);
    app.build_frame().ok()?;
    let node = app.tree().sole_node_with_key(hash_key(Key::from(key)))?;
    let rest = app.tree().visual_rect(node)?;
    let pointer = PointerEvent {
        contact: 0,
        kind: PointerKind::Mouse,
        position: Point::new(rest.x + rest.w / 2.0, rest.y + rest.h / 2.0),
        buttons: 1,
    };
    app.handle_input(&InputEvent::PointerDown(pointer, PointerButton::Primary));
    app.build_frame().ok()?;
    let record = app.tree().get(node)?;
    match record.paint().transform {
        Some(shift) if shift != aqua_core::Transform2D::IDENTITY => Some(Press::Moves),
        _ => Some(Press::HoldsStill),
    }
}

fn close_button(theme: &ArtisanTheme) -> aqua_ui::element::Element {
    IconButton::new(
        "close",
        *theme,
        AssetId::TABLER_X,
        "Close",
        IconButtonSize::Sm,
        Finish::Glass,
    )
    .on_activate(|_| {})
    .build()
}

#[test]
fn pressing_a_button_holds_it_still_under_reduced_motion() {
    assert_eq!(
        press(Motion::Full, "close", close_button),
        Some(Press::Moves)
    );
    assert_eq!(
        press(Motion::Reduced, "close", close_button),
        Some(Press::HoldsStill)
    );
}
