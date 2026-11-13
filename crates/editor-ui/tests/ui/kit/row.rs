use aqua_core::{Point, Size};
use aqua_reactive::Runtime;
use aqua_ui::binding::View;
use aqua_ui::element::{Box as UiBox, Element, Key};
use aqua_ui::events::hit_test;
use aqua_ui::layout::NaiveMeasurer;
use aqua_ui::tree::{UiTree, hash_key};
use editor_ui::kit::{Density, Radius, Row, RowItem, Space, Surface, Tooltip};
use editor_ui::theme::{ArtisanTheme, ThemeMode};

#[test]
fn a_menu_item_is_the_account_menu_item() {
    assert_eq!(Row::MenuItem.height(), Some(36.0), "36 high");
    assert_eq!(Row::MenuItem.pad().x, Space::S12, "12 across");
    assert_eq!(Row::MenuItem.pad().y, Space::S8, "8 down");
    assert_eq!(
        Row::MenuItem.text_inset(Surface::Menu),
        18.0,
        "text sits 18 from the edge"
    );
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
fn list_headings_align_with_list_item_text() {
    for density in [Density::Regular, Density::Tall] {
        assert_eq!(
            Row::ListHeading.text_inset(Surface::Inset),
            Row::ListItem(density).text_inset(Surface::Inset),
            "a heading in a list lines up with the rows below it"
        );
    }
    assert_eq!(
        Row::ListHeading.height(),
        Row::Heading.height(),
        "list headings are as tall as menu headings"
    );
    assert!(
        !Row::ListHeading.interactive(),
        "list headings are labels, not targets"
    );
}

#[test]
fn heights_are_lines_plus_padding() {
    assert_eq!(
        Row::ListItem(Density::Regular).height(),
        Some(36.0),
        "regular"
    );
    assert_eq!(Row::ListItem(Density::Dense).height(), Some(28.0), "dense");
    assert_eq!(
        Row::ListItem(Density::Tall).height(),
        Some(52.0),
        "tall carries a caption"
    );
    assert_eq!(Row::Heading.height(), Some(28.0), "heading");
    assert_eq!(
        Row::FormRow.height(),
        None,
        "form rows size to their control"
    );
}

#[test]
fn rows_outside_a_nesting_surface_fall_back_to_the_concentric_rule() {
    let radius = Row::MenuItem.radius_in(Surface::Tooltip(Tooltip::Card));
    assert_eq!(
        radius, 0.0,
        "16 inside 10.4 leaves square corners, never a guessed radius"
    );
}

fn control() -> Element {
    UiBox::new().width(40.0).height(20.0).build().with_key("control")
}

fn hits_control(row: Row) -> bool {
    let runtime = Runtime::new();
    let mut tree = UiTree::new();
    let mut mounted = View::new(runtime.create_scope(), tree.command_sink());
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    let item = RowItem::new("row", *theme, row, Surface::Card, "Title")
        .detail("A description long enough to need room")
        .trailing(control())
        .build();
    assert!(mounted.set_content(&mut tree, vec![item]).is_ok(), "the row mounts");
    assert!(
        mounted
            .compute_layout(&mut tree, Size::new(400.0, 200.0), &NaiveMeasurer)
            .is_ok(),
        "the row lays out"
    );
    let control = tree.sole_node_with_key(hash_key(Key::from("control")));
    let rect = control.and_then(|node| tree.rect(node));
    let (Some(control), Some(rect)) = (control, rect) else {
        return false;
    };
    let mut hit = hit_test(&tree, Point::new(rect.x + rect.w / 2.0, rect.y + rect.h / 2.0));
    while let Some(node) = hit {
        if node == control {
            return true;
        }
        hit = tree.get(node).and_then(|record| record.parent);
    }
    false
}

#[test]
fn a_form_rows_trailing_control_receives_the_pointer() {
    assert!(
        hits_control(Row::FormRow),
        "a switch or select at the end of a form row stays clickable"
    );
}

#[test]
fn only_form_rows_wrap_and_skip_the_hover() {
    let static_rows = [Row::FormRow, Row::Heading];
    for row in static_rows {
        assert!(!row.interactive(), "{row:?} carries no hover or activation");
    }
    assert!(Row::FormRow.wraps(), "form descriptions wrap instead of truncating");
    assert!(!Row::MenuItem.wraps(), "menu items stay on one line");
}

fn hover_lights(item: RowItem) -> bool {
    let mut app = aqua::UiApp::new(aqua_text::SharedFonts::new(), aqua_text::SharedRuns::new());
    assert!(app.mount(move |_| item.build()).is_ok(), "the row mounts");
    app.set_viewport(400.0, 200.0, 1.0);
    assert!(app.build_frame().is_ok(), "the row lays out");
    let row = app.tree().sole_node_with_key(hash_key(Key::from("row")));
    let Some(rect) = row.and_then(|node| app.tree().rect(node)) else {
        return false;
    };
    editor_ui::kit::live::advance(&mut app, std::time::Instant::now(), true);
    assert!(
        !editor_ui::kit::live::advance(&mut app, std::time::Instant::now(), true).changed,
        "nothing lights before the pointer arrives"
    );
    app.handle_input(&aqua_core::InputEvent::PointerMove(
        aqua_core::PointerEvent {
            contact: 0,
            kind: aqua_core::PointerKind::Mouse,
            position: Point::new(rect.x + rect.w / 2.0, rect.y + rect.h / 2.0),
            buttons: 0,
        },
    ));
    editor_ui::kit::live::advance(&mut app, std::time::Instant::now(), true).changed
}

#[test]
fn a_row_with_a_hover_card_still_lights_on_hover() {
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    let row =
        || RowItem::new("row", *theme, Row::MenuItem, Surface::Menu, "Agent").on_activate(|| {});
    assert!(hover_lights(row()), "a plain row lights");
    assert!(
        hover_lights(row().hint(|| UiBox::new().build())),
        "a hint card sits beside the hover instead of replacing it"
    );
}
