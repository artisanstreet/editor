use aqua_core::{Point, Size};
use aqua_reactive::Runtime;
use aqua_ui::binding::View;
use aqua_ui::element::{Box as UiBox, Element, Key};
use aqua_ui::events::hit_test;
use aqua_ui::layout::NaiveMeasurer;
use aqua_ui::tree::{UiTree, hash_key};
use editor_ui::kit::{Density, Row, RowItem, Surface};
use editor_ui::theme::{ArtisanTheme, ThemeMode};

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

fn control() -> Element {
    UiBox::new()
        .width(40.0)
        .height(20.0)
        .build()
        .with_key("control")
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
    assert!(
        mounted.set_content(&mut tree, vec![item]).is_ok(),
        "the row mounts"
    );
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
    let mut hit = hit_test(
        &tree,
        Point::new(rect.x + rect.w / 2.0, rect.y + rect.h / 2.0),
    );
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
        hits_control(Row::Form),
        "a switch or select at the end of a form row stays clickable"
    );
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

pub(crate) fn painted_with_keyboard_focus(element: Element, key: &str) -> (usize, usize) {
    let mut app = aqua::UiApp::new(aqua_text::SharedFonts::new(), aqua_text::SharedRuns::new());
    assert!(app.mount(move |_| element).is_ok(), "the control mounts");
    app.set_viewport(400.0, 200.0, 1.0);
    assert!(app.build_frame().is_ok(), "the control lays out");
    let Some(node) = app.tree().sole_node_with_key(hash_key(Key::from(key))) else {
        return (0, 0);
    };
    let painted = |app: &mut aqua::UiApp| {
        app.tree_mut()
            .emit_scene()
            .into_iter()
            .filter(|(owner, _)| *owner == node)
            .count()
    };
    let resting = painted(&mut app);
    let tab = aqua_core::KeyEvent {
        key: aqua_core::Key::Named(aqua_core::NamedKey::Tab),
        modifiers: aqua_core::Modifiers::default(),
        repeat: false,
    };
    app.handle_input(&aqua_core::InputEvent::KeyDown(tab));
    app.handle_input(&aqua_core::InputEvent::KeyUp(tab));
    assert_eq!(app.tree().focused(), Some(node), "Tab reaches the control");
    assert!(app.build_frame().is_ok(), "the focused control lays out");
    (resting, painted(&mut app))
}

#[test]
fn a_row_reached_with_tab_shows_a_focus_ring() {
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    let row = RowItem::new("row", *theme, Row::MenuItem, Surface::Menu, "Agent")
        .on_activate(|| {})
        .build();
    let (resting, focused) = painted_with_keyboard_focus(row, "row");
    assert!(
        focused > resting,
        "keyboard focus paints an indicator ({resting} items at rest, {focused} focused)"
    );
}
