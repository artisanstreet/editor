use editor_ui::kit::{Density, Radius, Row, Space, Surface, Tooltip};

#[test]
fn a_menu_item_is_the_account_menu_item() {
    assert_eq!(Row::MenuItem.height(), Some(36.0), "36 high");
    assert_eq!(Row::MenuItem.pad().x, Space::S12, "12 across");
    assert_eq!(Row::MenuItem.pad().y, Space::S8, "8 down");
    assert_eq!(Row::MenuItem.text_inset(Surface::Menu), 18.0, "text sits 18 from the edge");
    assert!(
        (Row::MenuItem.radius_in(Surface::Menu) - Radius::Row.px()).abs() < 1e-4,
        "rows take the menu's concentric radius"
    );
}

#[test]
fn headings_align_with_menu_item_text() {
    assert_eq!(
        Row::Heading.text_inset(Surface::Menu),
        Row::MenuItem.text_inset(Surface::Menu),
        "headings line up with the items they label"
    );
}

#[test]
fn heights_are_lines_plus_padding() {
    assert_eq!(Row::ListItem(Density::Regular).height(), Some(36.0), "regular");
    assert_eq!(Row::ListItem(Density::Dense).height(), Some(28.0), "dense");
    assert_eq!(Row::ListItem(Density::Tall).height(), Some(52.0), "tall carries a caption");
    assert_eq!(Row::Heading.height(), Some(28.0), "heading");
    assert_eq!(Row::FormRow.height(), None, "form rows size to their control");
}

#[test]
fn rows_outside_a_nesting_surface_fall_back_to_the_concentric_rule() {
    let radius = Row::MenuItem.radius_in(Surface::Tooltip(Tooltip::Card));
    assert_eq!(radius, 0.0, "16 inside 10.4 leaves square corners, never a guessed radius");
}
