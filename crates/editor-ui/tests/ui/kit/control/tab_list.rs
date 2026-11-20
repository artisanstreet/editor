use aqua_core::{InputEvent, Key, KeyEvent, Modifiers, NamedKey, Role};
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::element::Box as UiBox;
use editor_ui::kit::{Orientation, TabList};
use editor_ui::theme::ArtisanTheme;
use std::{cell::RefCell, rc::Rc};

fn pressed(orientation: Orientation, keys: &[NamedKey]) -> Vec<usize> {
    let chosen = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&chosen);
    let list = ["Model", "Effort", "Speed"]
        .into_iter()
        .enumerate()
        .fold(
            TabList::new(
                "panes",
                "Settings",
                orientation,
                UiBox::new().layout("column"),
                move |index| sink.borrow_mut().push(index),
            ),
            |list, (index, name)| {
                list.tab(
                    UiBox::new()
                        .width(80.0)
                        .height(20.0)
                        .build()
                        .aria_label(name),
                    index == 1,
                )
            },
        )
        .build(&ArtisanTheme::resolved());
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| list).expect("the tab list mounts");
    app.set_viewport(200.0, 200.0, 1.0);
    app.build_frame().expect("the tab list lays out");
    for key in std::iter::once(&NamedKey::Tab).chain(keys) {
        let event = KeyEvent {
            key: Key::Named(*key),
            modifiers: Modifiers::NONE,
            repeat: false,
        };
        app.handle_input(&InputEvent::KeyDown(event));
        app.handle_input(&InputEvent::KeyUp(event));
    }
    let semantics = app.tree().emit_semantics();
    let selected: Vec<_> = semantics
        .iter()
        .filter(|node| node.role == Role::Tab && node.selected)
        .map(|node| node.label.clone())
        .collect();
    assert_eq!(selected, ["Effort"], "the selected tab is marked selected");
    let chosen = chosen.take();
    let focused = semantics
        .iter()
        .find(|node| node.focused)
        .map(|node| node.label.clone());
    let expected = ["Model", "Effort", "Speed"][chosen.last().copied().unwrap_or(1)];
    assert_eq!(
        focused.as_deref(),
        Some(expected),
        "focus follows the chosen tab"
    );
    chosen
}

#[test]
fn a_vertical_tab_list_moves_with_up_down_home_and_end_and_wraps() {
    assert_eq!(
        pressed(
            Orientation::Vertical,
            &[
                NamedKey::Down,
                NamedKey::Down,
                NamedKey::Up,
                NamedKey::Home,
                NamedKey::Up,
                NamedKey::End
            ]
        ),
        [2, 0, 2, 0, 2, 2]
    );
}

#[test]
fn a_horizontal_tab_list_ignores_up_and_down() {
    assert_eq!(
        pressed(
            Orientation::Horizontal,
            &[NamedKey::Down, NamedKey::Right, NamedKey::Left]
        ),
        [2, 1]
    );
}
