use super::icon_button::{Press, press};
use editor_ui::code_copy::{CodeCopyButton, CodeCopyController};
use editor_ui::theme::{ArtisanTheme, Motion};

fn copy_button(theme: &ArtisanTheme) -> aqua_ui::element::Element {
    let reduced = theme.interaction.motion == Motion::Reduced;
    CodeCopyButton::new(
        "copy",
        "fn main() {}",
        *theme,
        CodeCopyController::default(),
        reduced,
        |_| {},
    )
    .build(std::time::Instant::now())
}

#[test]
fn pressing_a_code_copy_button_holds_it_still_under_reduced_motion() {
    assert_eq!(press(Motion::Full, "copy", copy_button), Some(Press::Moves));
    assert_eq!(
        press(Motion::Reduced, "copy", copy_button),
        Some(Press::HoldsStill)
    );
}
