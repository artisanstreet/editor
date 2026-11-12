use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
};

use aqua_core::{InputEvent, Key, Modifiers, NamedKey};
use aqua_ui::{
    element::{Box as UiBox, Element},
    input::{EventPhase, InputKind, InputResponse, RoutedInput},
};

use super::Palette;
use crate::{
    core::Screen,
    ui::{
        dialog::{Dialog, DialogDismissReason},
        kit::{Density, Row, RowItem, Space, Spaced, Surface, TextRole},
        text::{
            Editor, Mode,
            mount::{EditorController, EditorEvent},
        },
        theme::ArtisanTheme,
    },
};

static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);
const QUERY_KEY: &str = "command-palette-query";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    QueryChanged(String),
    PresentationChanged,
    Up,
    Down,
    Submit,
    Select(Screen),
    Dismiss,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub session: u64,
    pub action: Action,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    None,
    Search(String),
    Close(Option<Screen>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub changed: bool,
    pub effect: Effect,
}

pub struct PaletteView {
    editor: EditorController,
    session: u64,
    active: Rc<Cell<bool>>,
}

impl PaletteView {
    pub fn new(emit: Rc<dyn Fn(Event)>) -> Self {
        let session = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
        let active = Rc::new(Cell::new(true));
        let query = Rc::new(RefCell::new(Editor::new("", Mode::SingleLine)));
        let weak_query = Rc::downgrade(&query);
        let weak_active = Rc::downgrade(&active);
        let editor = EditorController::from_shared(query, move |event| {
            if !weak_active.upgrade().is_some_and(|active| active.get()) {
                return;
            }
            let action = match event {
                EditorEvent::Changed => {
                    let Some(query) = weak_query.upgrade() else {
                        return;
                    };
                    Action::QueryChanged(query.borrow().text().to_owned())
                }
                EditorEvent::PresentationChanged => Action::PresentationChanged,
                EditorEvent::Submitted => Action::Submit,
                EditorEvent::SelectionChanged { .. }
                | EditorEvent::List(_)
                | EditorEvent::PastedFiles { .. } => return,
            };
            emit(Event { session, action });
        });
        Self {
            editor,
            session,
            active,
        }
    }

    pub fn session(&self) -> u64 {
        self.session
    }

    pub fn close(&self) {
        self.active.set(false);
    }

    pub fn focus_query(&self, app: &mut aqua::UiApp) -> bool {
        self.active.get() && app.focus_key(QUERY_KEY)
    }

    pub fn sync_query(&self, app: &mut aqua::UiApp) -> bool {
        if !self.active.get() {
            return false;
        }
        let hash = aqua_ui::tree::hash_key(aqua_ui::element::Key::from(QUERY_KEY));
        app.tree()
            .sole_node_with_key(hash)
            .is_some_and(|node| self.editor.sync(app.tree_mut(), node))
    }

    pub fn dispatch(&self, palette: &mut Palette, event: Event) -> Outcome {
        if !self.active.get() || event.session != self.session {
            return Outcome {
                changed: false,
                effect: Effect::None,
            };
        }
        match event.action {
            Action::QueryChanged(query) => {
                if palette.query == query {
                    Outcome {
                        changed: false,
                        effect: Effect::None,
                    }
                } else {
                    Outcome {
                        changed: true,
                        effect: Effect::Search(query),
                    }
                }
            }
            Action::PresentationChanged => Outcome {
                changed: true,
                effect: Effect::None,
            },
            Action::Up => {
                let before = palette.highlighted;
                palette.up();
                Outcome {
                    changed: before != palette.highlighted,
                    effect: Effect::None,
                }
            }
            Action::Down => {
                let before = palette.highlighted;
                palette.down();
                Outcome {
                    changed: before != palette.highlighted,
                    effect: Effect::None,
                }
            }
            Action::Submit => Outcome {
                changed: true,
                effect: Effect::Close(palette.selected().cloned()),
            },
            Action::Select(screen) => Outcome {
                changed: true,
                effect: Effect::Close(Some(screen)),
            },
            Action::Dismiss => Outcome {
                changed: true,
                effect: Effect::Close(None),
            },
        }
    }

    pub fn content(
        &self,
        palette: &Palette,
        theme: &ArtisanTheme,
        emit: Rc<dyn Fn(Event)>,
    ) -> Element {
        let query = self.editor.element_with_notice_keyed(
            QUERY_KEY,
            Mode::SingleLine,
            theme,
            "Search threads and commands",
            false,
            false,
        );
        let mut content = UiBox::new().layout("column").spaced(Space::S4).child(query);
        if let Some(error) = &palette.search_error {
            content = content.child(TextRole::Caption.text(theme, error.clone()).build());
        }
        for (index, entry) in palette.entries.iter().enumerate() {
            let screen = entry.screen.clone();
            let callback = guarded(self.session, &self.active, emit.clone());
            content = content.child(
                row(theme, index, entry, index == palette.highlighted)
                    .on_activate(move || callback(Action::Select(screen.clone())))
                    .build(),
            );
        }
        let callback = guarded(self.session, &self.active, emit);
        content
            .build()
            .on_input(InputKind::KeyDown, EventPhase::Capture, move |event| {
                let RoutedInput::Native(InputEvent::KeyDown(key)) = &event.input else {
                    return InputResponse::default();
                };
                if key.modifiers != Modifiers::default() {
                    return InputResponse::default();
                }
                let action = match key.key {
                    Key::Named(NamedKey::Escape) => Action::Dismiss,
                    Key::Named(NamedKey::Up) => Action::Up,
                    Key::Named(NamedKey::Down) => Action::Down,
                    _ => return InputResponse::default(),
                };
                callback(action);
                InputResponse {
                    stop_propagation: true,
                    ..InputResponse::handled()
                }
            })
    }

    pub fn dialog(
        &self,
        palette: &Palette,
        theme: &ArtisanTheme,
        emit: Rc<dyn Fn(Event)>,
    ) -> Dialog {
        let dismiss = guarded(self.session, &self.active, emit.clone());
        Dialog::new(
            "command-palette",
            *theme,
            "Search threads and commands",
            self.content(palette, theme, emit),
        )
        .on_dismiss(move |_: DialogDismissReason| dismiss(Action::Dismiss))
    }
}

impl Drop for PaletteView {
    fn drop(&mut self) {
        self.close();
    }
}

fn row(theme: &ArtisanTheme, index: usize, entry: &super::Entry, selected: bool) -> RowItem {
    let row = match entry.detail.is_empty() {
        true => RowItem::new(
            index,
            *theme,
            Row::ListItem(Density::Regular),
            Surface::Dialog,
            entry.label.clone(),
        ),
        false => RowItem::new(
            index,
            *theme,
            Row::ListItem(Density::Tall),
            Surface::Dialog,
            entry.label.clone(),
        )
        .detail(entry.detail.clone()),
    };
    row.selected(selected)
}

fn guarded(
    session: u64,
    active: &Rc<Cell<bool>>,
    emit: Rc<dyn Fn(Event)>,
) -> impl Fn(Action) + 'static {
    let active = Rc::downgrade(active);
    move |action| {
        if active.upgrade().is_some_and(|active| active.get()) {
            emit(Event { session, action });
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/shell/palette/view.rs"]
mod tests;
