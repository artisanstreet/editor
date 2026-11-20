use std::cell::{Cell, RefCell};
use std::rc::Rc;

use aqua_core::{InputEvent, Key as InputKey, NamedKey, NodeId, Role};
use aqua_ui::element::{Box as UiBox, Canvas, Element, Key};
use aqua_ui::input::{EventPhase, InputKind, InputResponse, RoutedInput};
use aqua_ui::style::Style;
use aqua_ui::tree::UiCommand;

use super::focus_ring;
use crate::ui::theme::ArtisanTheme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Orientation {
    Horizontal,
    Vertical,
}

impl Orientation {
    fn step(self, key: NamedKey) -> Option<Step> {
        let (previous, next) = match self {
            Self::Horizontal => (NamedKey::Left, NamedKey::Right),
            Self::Vertical => (NamedKey::Up, NamedKey::Down),
        };
        [
            (previous, Step::Previous),
            (next, Step::Next),
            (NamedKey::Home, Step::First),
            (NamedKey::End, Step::Last),
        ]
        .into_iter()
        .find_map(|(bound, step)| (bound == key).then_some(step))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Previous,
    Next,
    First,
    Last,
}

impl Step {
    const fn from(self, at: usize, count: usize) -> usize {
        match self {
            Self::Previous => (at + count - 1) % count,
            Self::Next => (at + 1) % count,
            Self::First => 0,
            Self::Last => count - 1,
        }
    }
}

pub struct TabList {
    key: Key,
    label: String,
    orientation: Orientation,
    container: UiBox,
    tabs: Vec<Element>,
    selected: Option<usize>,
    select: Rc<dyn Fn(usize)>,
}

impl std::fmt::Debug for TabList {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TabList")
            .field("label", &self.label)
            .field("orientation", &self.orientation)
            .field("selected", &self.selected)
            .finish_non_exhaustive()
    }
}

impl TabList {
    #[must_use]
    pub fn new(
        key: impl Into<Key>,
        label: impl Into<String>,
        orientation: Orientation,
        container: UiBox,
        select: impl Fn(usize) + 'static,
    ) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            orientation,
            container,
            tabs: Vec::new(),
            selected: None,
            select: Rc::new(select),
        }
    }

    #[must_use]
    pub fn tab(mut self, tab: Element, selected: bool) -> Self {
        if selected {
            self.selected = Some(self.tabs.len());
        }
        self.tabs.push(tab);
        self
    }

    #[must_use]
    pub fn build(self, theme: &ArtisanTheme) -> Element {
        let count = self.tabs.len();
        let selected = self.selected;
        let ring = focus_ring(theme);
        let tabs = self.tabs.into_iter().enumerate().map(|(index, tab)| {
            tab.role(Role::Tab)
                .aria_selected(selected == Some(index))
                .focusable(true)
                .tab_index(if selected.unwrap_or(0) == index {
                    0
                } else {
                    -1
                })
                .focus_visible_style(ring.clone())
        });
        let shown: Rc<RefCell<Vec<NodeId>>> = Rc::new(RefCell::new(Vec::new()));
        let seen = Rc::clone(&shown);
        let reader = Canvas::new()
            .style(Style::new().absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0)))
            .on_prepaint(move |tree, node| {
                let tabs = tree
                    .get(node)
                    .and_then(|reader| reader.parent)
                    .and_then(|list| tree.get(list))
                    .map(|list| list.children.iter().take(count).copied().collect())
                    .unwrap_or_default();
                *seen.borrow_mut() = tabs;
            })
            .build()
            .exclude_from_hit_testing()
            .aria_hidden();
        let current = Rc::new(Cell::new(selected.unwrap_or(0)));
        let select = self.select;
        let orientation = self.orientation;
        self.container
            .children(tabs)
            .child(reader)
            .build()
            .with_key(self.key)
            .role(Role::TabList)
            .aria_label(self.label)
            .on_input(InputKind::KeyDown, EventPhase::Bubble, move |event| {
                let RoutedInput::Native(InputEvent::KeyDown(key)) = &event.input else {
                    return InputResponse::default();
                };
                let step = match key.key {
                    InputKey::Named(named) => orientation.step(named),
                    InputKey::Char(_) => None,
                };
                let Some(step) = step.filter(|_| count > 0) else {
                    return InputResponse::default();
                };
                let at = shown
                    .borrow()
                    .iter()
                    .position(|tab| *tab == event.target)
                    .unwrap_or_else(|| current.get());
                let next = step.from(at, count);
                current.set(next);
                select(next);
                InputResponse {
                    stop_propagation: true,
                    commands: shown
                        .borrow()
                        .get(next)
                        .map(|tab| UiCommand::RequestFocus { node: Some(*tab) })
                        .into_iter()
                        .collect(),
                    ..InputResponse::handled()
                }
            })
    }
}
