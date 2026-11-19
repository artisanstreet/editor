use super::*;
use aqua_core::{InputEvent, Key as InputKey, KeyEvent, Modifiers, NamedKey, Rect, SemanticNode};
use aqua_text::{SharedFonts, SharedRuns};

const TRIGGER: &str = "long-select";

fn entries() -> Vec<SelectEntry<usize>> {
    (0..30)
        .map(|index| SelectEntry::item(index, format!("Option {index}")))
        .collect()
}

fn mounted(view: Element) -> aqua::UiApp {
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| view).unwrap();
    app.set_viewport(400.0, 800.0, 1.0);
    app.build_frame().unwrap();
    app
}

fn trigger(state: &Rc<RefCell<SelectState<usize>>>) -> Element {
    Select::new(
        TRIGGER,
        ArtisanTheme::resolved(),
        Some(0),
        entries(),
        Rc::clone(state),
    )
    .label("Long")
    .build()
}

fn menu(state: &Rc<RefCell<SelectState<usize>>>) -> Element {
    SelectMenu::new(
        ArtisanTheme::resolved(),
        Some(0),
        entries(),
        Rc::clone(state),
        240.0,
    )
    .build()
}

fn arrow_down(state: &Rc<RefCell<SelectState<usize>>>, presses: usize) {
    let mut app = mounted(trigger(state));
    state.borrow_mut().set_open(true);
    assert!(app.focus_key(TRIGGER));
    let event = KeyEvent {
        key: InputKey::Named(NamedKey::Down),
        modifiers: Modifiers::default(),
        repeat: false,
    };
    for _ in 0..presses {
        app.handle_input(&InputEvent::KeyDown(event));
        app.handle_input(&InputEvent::KeyUp(event));
    }
}

fn node(app: &aqua::UiApp, role: Role, label: &str) -> SemanticNode {
    app.tree()
        .emit_semantics()
        .into_iter()
        .find(|node| node.role == role && node.label == label)
        .unwrap_or_else(|| panic!("no {role:?} named {label:?}"))
}

fn bounds(node: &SemanticNode) -> Rect {
    node.bounds
        .unwrap_or_else(|| panic!("{:?} has no bounds", node.label))
}

#[test]
fn arrowing_past_the_visible_options_scrolls_the_highlighted_option_into_view() {
    let state = Rc::new(RefCell::new(SelectState::new(Instant::now())));
    arrow_down(&state, 12);
    assert_eq!(state.borrow().highlighted(), Some(&12));
    let app = mounted(menu(&state));
    let list = bounds(
        &app.tree()
            .emit_semantics()
            .into_iter()
            .find(|node| node.role == Role::ListBox)
            .expect("the menu is a list box"),
    );
    let option = bounds(&node(&app, Role::Option, "Option 12"));
    assert!(
        option.y >= list.y && option.y + option.h <= list.y + list.h,
        "Option 12 at {option:?} is inside the menu at {list:?}"
    );
}

#[test]
fn a_long_menu_exposes_its_scroll_range_to_assistive_tech() {
    let state = Rc::new(RefCell::new(SelectState::new(Instant::now())));
    let app = mounted(menu(&state));
    let scroll = app
        .tree()
        .emit_semantics()
        .into_iter()
        .find_map(|node| node.scroll)
        .expect("the option list is a scroll region");
    assert!(scroll.y_max > 0.0, "options run past the menu's height");
    assert!(scroll.y.abs() < f64::EPSILON);
}

#[test]
fn the_keyboard_highlighted_option_is_the_triggers_active_descendant() {
    let state = Rc::new(RefCell::new(SelectState::new(Instant::now())));
    arrow_down(&state, 2);
    let mut app = mounted(
        UiBox::new()
            .layout("column")
            .child(trigger(&state))
            .child(menu(&state))
            .build(),
    );
    assert!(app.focus_key(TRIGGER));
    let option = node(&app, Role::Option, "Option 2");
    let combo = app
        .tree()
        .emit_semantics()
        .into_iter()
        .find(|node| node.role == Role::ComboBox)
        .expect("the trigger is a combo box");
    assert_eq!(combo.active_descendant, Some(option.node));
}
