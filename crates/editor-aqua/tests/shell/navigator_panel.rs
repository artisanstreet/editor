use aqua_core::Point;
use aqua_ui::element::Box as UiBox;
use artisan_api::{ItemId, OutlineEntry, Seq};
use editor_aqua::conversation::view::{Dispatch, navigator};
use editor_aqua::ui::theme::ArtisanTheme;
use std::cell::RefCell;
use std::rc::Rc;

fn outline(count: usize) -> Vec<OutlineEntry> {
    (0..count)
        .map(|index| OutlineEntry {
            id: ItemId::from(format!("item-{index}")),
            position: index as Seq * 10,
            label: format!("turn {index}"),
        })
        .collect()
}

fn mounted_panel(count: usize, jumps: Rc<RefCell<Vec<Seq>>>) -> aqua::UiApp {
    use aqua_text::{SharedFonts, SharedRuns};
    use editor_aqua::conversation::adapter::Action;
    let markers = outline(count);
    let sink = jumps;
    let dispatch: Dispatch = Rc::new(move |action| {
        if let Action::Jump(position) = action {
            sink.borrow_mut().push(position);
        }
    });
    let menu = navigator::panel(
        &markers,
        None,
        None,
        &ArtisanTheme::resolved(),
        1.0,
        dispatch,
    );
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| UiBox::new().width(900.0).height(700.0).child(menu).build())
        .unwrap();
    app.set_viewport(900.0, 700.0, 1.0);
    app.build_frame().unwrap();
    app
}

fn row_center(app: &aqua::UiApp, id: &str) -> Point {
    let key = aqua_ui::tree::hash_key(aqua_ui::element::Key::from(format!(
        "navigator-row-hit-{id}"
    )));
    let node = app.tree().sole_node_with_key(key).expect("row mounted");
    let bounds = app.tree().visual_rect(node).expect("row laid out");
    Point::new(bounds.x + bounds.w / 2.0, bounds.y + bounds.h / 2.0)
}

fn press_and_release(app: &mut aqua::UiApp, at: Point) {
    use aqua_core::{InputEvent, PointerButton, PointerEvent, PointerKind};
    let pointer = PointerEvent {
        contact: 0,
        kind: PointerKind::Mouse,
        position: at,
        buttons: 1,
    };
    app.handle_input(&InputEvent::PointerMove(pointer));
    app.handle_input(&InputEvent::PointerDown(pointer, PointerButton::Primary));
    app.handle_input(&InputEvent::PointerUp(pointer, PointerButton::Primary));
}

#[test]
fn clicking_a_panel_row_jumps_to_its_turn() {
    let jumps = Rc::new(RefCell::new(Vec::new()));
    let mut app = mounted_panel(8, Rc::clone(&jumps));
    let at = row_center(&app, "item-3");
    press_and_release(&mut app, at);
    assert_eq!(&*jumps.borrow(), &[30]);
}

#[test]
fn clicking_a_row_of_the_scrolled_panel_jumps_to_its_turn() {
    use aqua_ui::tree::NodeKind;
    let jumps = Rc::new(RefCell::new(Vec::new()));
    let mut app = mounted_panel(40, Rc::clone(&jumps));
    let scroll = app
        .tree()
        .traverse()
        .into_iter()
        .find(|node| {
            matches!(
                app.tree().get(*node).map(|r| &r.kind),
                Some(NodeKind::Scroll(_))
            )
        })
        .expect("panel list scrolls");
    assert!(app.tree_mut().scroll_to(scroll, Point::new(0.0, 360.0)));
    app.build_frame().unwrap();
    let at = row_center(&app, "item-12");
    press_and_release(&mut app, at);
    assert_eq!(&*jumps.borrow(), &[120]);
}
