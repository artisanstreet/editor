use aqua::UiApp;
use aqua_core::{InputEvent, Key, NamedKey, NodeId};

pub const POPOVER_MENU: &str = "popover-menu";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction {
    Forward,
    Backward,
}

#[must_use]
pub const fn next_index(at: Option<usize>, count: usize, direction: Direction) -> Option<usize> {
    if count == 0 {
        return None;
    }
    Some(match (at, direction) {
        (None, Direction::Forward) => 0,
        (None, Direction::Backward) => count - 1,
        (Some(index), Direction::Forward) => (index + 1) % count,
        (Some(index), Direction::Backward) => (index + count - 1) % count,
    })
}

fn is_menu(ui: &UiApp, node: NodeId) -> bool {
    ui.tree()
        .get(node)
        .is_some_and(|record| record.debug_selector.as_deref() == Some(POPOVER_MENU))
}

fn menu_of(ui: &UiApp, focused: NodeId) -> Option<NodeId> {
    let mut node = focused;
    loop {
        if is_menu(ui, node) {
            return Some(node);
        }
        node = ui.tree().get(node)?.parent?;
    }
}

fn menu_below(ui: &UiApp, root: NodeId) -> Option<NodeId> {
    ui.tree()
        .traverse()
        .into_iter()
        .find(|node| is_menu(ui, *node) && within(ui, *node, root))
}

fn within(ui: &UiApp, mut node: NodeId, menu: NodeId) -> bool {
    loop {
        if node == menu {
            return true;
        }
        match ui.tree().get(node).and_then(|record| record.parent) {
            Some(parent) => node = parent,
            None => return false,
        }
    }
}

pub fn cycle(ui: &mut UiApp, input: &InputEvent) -> bool {
    let InputEvent::KeyDown(key) = input else {
        return false;
    };
    if !key.modifiers.is_empty() {
        return false;
    }
    let direction = match key.key {
        Key::Named(NamedKey::Down) => Direction::Forward,
        Key::Named(NamedKey::Up) => Direction::Backward,
        _ => return false,
    };
    let Some(focused) = ui.tree().focused() else {
        return false;
    };
    let Some(menu) = menu_of(ui, focused).or_else(|| menu_below(ui, focused)) else {
        return false;
    };
    let items: Vec<_> = aqua_ui::events::focusable_nodes(ui.tree())
        .into_iter()
        .filter(|node| within(ui, *node, menu))
        .collect();
    let at = items.iter().position(|node| *node == focused);
    let Some(next) = next_index(at, items.len(), direction) else {
        return false;
    };
    ui.tree_mut().set_focused(Some(items[next]));
    true
}
