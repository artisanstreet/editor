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
    trigger_in(&ArtisanTheme::resolved(), state)
}

fn trigger_in(theme: &ArtisanTheme, state: &Rc<RefCell<SelectState<usize>>>) -> Element {
    Select::new(TRIGGER, *theme, Some(0), entries(), Rc::clone(state))
        .label("Long")
        .build()
}

fn menu(state: &Rc<RefCell<SelectState<usize>>>) -> Element {
    menu_in(&ArtisanTheme::resolved(), entries(), state)
}

fn menu_in(
    theme: &ArtisanTheme,
    entries: Vec<SelectEntry<usize>>,
    state: &Rc<RefCell<SelectState<usize>>>,
) -> Element {
    SelectMenu::new(*theme, Some(0), entries, Rc::clone(state), 240.0).build()
}

fn arrow_down(state: &Rc<RefCell<SelectState<usize>>>, presses: usize) {
    arrow_down_in(&ArtisanTheme::resolved(), state, presses);
}

fn arrow_down_in(theme: &ArtisanTheme, state: &Rc<RefCell<SelectState<usize>>>, presses: usize) {
    let mut app = mounted(trigger_in(theme, state));
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

fn menu_scroll(app: &aqua::UiApp) -> (aqua_core::NodeId, f64) {
    app.tree()
        .emit_semantics()
        .into_iter()
        .find_map(|node| node.scroll.map(|scroll| (node.node, scroll.y)))
        .expect("the option list is a scroll region")
}

fn assistive_scroll(
    app: &mut aqua::UiApp,
    action: aqua_protocol::A11yAction,
    data: aqua_protocol::A11yActionData,
) {
    let (node, _) = menu_scroll(app);
    app.drive(aqua::RuntimeInput::Access(
        aqua_protocol::A11yActionRequest { node, action, data },
    ));
}

#[test]
fn a_screen_reader_scroll_of_the_menu_survives_a_rebuild() {
    use aqua_protocol::{A11yAction, A11yActionData, A11yScroll};
    let state = Rc::new(RefCell::new(SelectState::new(Instant::now())));
    let mut app = mounted(menu(&state));
    assistive_scroll(
        &mut app,
        A11yAction::Scroll(A11yScroll::Down),
        A11yActionData::None,
    );
    let (_, paged) = menu_scroll(&app);
    assert!(paged > 0.0, "a page down moves the option list");
    app.refresh(vec![menu(&state)]).unwrap();
    app.build_frame().unwrap();
    assert!(
        (menu_scroll(&app).1 - paged).abs() < 0.5,
        "the page down stays after a rebuild"
    );
    assistive_scroll(
        &mut app,
        A11yAction::SetScrollOffset,
        A11yActionData::ScrollOffset { x: 0.0, y: 64.0 },
    );
    app.build_frame().unwrap();
    app.refresh(vec![menu(&state)]).unwrap();
    app.build_frame().unwrap();
    assert!(
        (menu_scroll(&app).1 - 64.0).abs() < 0.5,
        "the set offset stays after a rebuild"
    );
    arrow_down(&state, 1);
    app.refresh(vec![menu(&state)]).unwrap();
    app.build_frame().unwrap();
    let list = bounds(
        &app.tree()
            .emit_semantics()
            .into_iter()
            .find(|node| node.role == Role::ListBox)
            .expect("the menu is a list box"),
    );
    let option = bounds(&node(&app, Role::Option, "Option 1"));
    assert!(
        option.y >= list.y && option.y + option.h <= list.y + list.h,
        "arrowing to Option 1 scrolls it back into view from where the screen reader left the list: {option:?} in {list:?}"
    );
}

fn largest_text() -> ArtisanTheme {
    let mut theme = ArtisanTheme::resolved();
    theme.typography = theme.typography.scaled(2.25);
    theme
}

fn text_rect(app: &aqua::UiApp, text: &str) -> Rect {
    use aqua_ui::tree::NodeKind;
    let tree = app.tree();
    tree.traverse()
        .into_iter()
        .find(|node| {
            tree.get(*node).is_some_and(
                |shown| matches!(&shown.kind, NodeKind::Text(shown) if shown.text() == text),
            )
        })
        .and_then(|node| tree.rect(node))
        .unwrap_or_else(|| panic!("{text:?} is shown"))
}

fn list_box(app: &aqua::UiApp) -> Rect {
    bounds(
        &app.tree()
            .emit_semantics()
            .into_iter()
            .find(|node| node.role == Role::ListBox)
            .expect("the menu is a list box"),
    )
}

#[test]
fn at_the_largest_text_size_each_option_is_tall_enough_for_its_label() {
    let state = Rc::new(RefCell::new(SelectState::new(Instant::now())));
    let three = entries().into_iter().take(3).collect();
    let app = mounted(menu_in(&largest_text(), three, &state));
    let option = bounds(&node(&app, Role::Option, "Option 1"));
    let label = text_rect(&app, "Option 1");
    let pad = 2.0 * Row::MenuItem.pad().y.px();
    assert!(
        label.h + pad <= option.h + 0.5,
        "the {}px label overflows its {}px option",
        label.h,
        option.h
    );
    let list = list_box(&app);
    let final_option = bounds(&node(&app, Role::Option, "Option 2"));
    assert!(
        final_option.y + final_option.h <= list.y + list.h + 0.5,
        "the menu at {list:?} cuts off its last option at {final_option:?}"
    );
}

#[test]
fn at_the_largest_text_size_arrowing_keeps_the_highlighted_option_in_view() {
    let state = Rc::new(RefCell::new(SelectState::new(Instant::now())));
    arrow_down_in(&largest_text(), &state, 12);
    assert_eq!(state.borrow().highlighted(), Some(&12));
    let app = mounted(menu_in(&largest_text(), entries(), &state));
    let list = list_box(&app);
    let option = bounds(&node(&app, Role::Option, "Option 12"));
    assert!(
        option.y >= list.y - 0.5 && option.y + option.h <= list.y + list.h + 0.5,
        "Option 12 at {option:?} is inside the menu at {list:?}"
    );
}
