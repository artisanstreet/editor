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

use super::{Palette, SearchProblem};
use crate::{
    core::{Screen, SettingsPage},
    ui::{
        kit::{
            Button, ButtonSize, ButtonTone, Density, Row, RowItem, Space, Spaced, Surface, TextRole,
        },
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
    RetrySearch,
    Dismiss,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub(crate) session: u64,
    pub(crate) action: Action,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    None,
    Search(String),
    Close(Option<Screen>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub(crate) changed: bool,
    pub(crate) effect: Effect,
}

pub struct PaletteView {
    editor: EditorController,
    session: u64,
    active: Rc<Cell<bool>>,
}

impl PaletteView {
    pub(crate) fn new(emit: Rc<dyn Fn(Event)>) -> Self {
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

    pub(crate) fn session(&self) -> u64 {
        self.session
    }

    pub(crate) fn close(&self) {
        self.active.set(false);
    }

    pub(crate) fn focus_query(&self, app: &mut aqua::UiApp) -> bool {
        self.active.get() && app.focus_key(QUERY_KEY)
    }

    pub(crate) fn sync_query(&self, app: &mut aqua::UiApp) -> bool {
        if !self.active.get() {
            return false;
        }
        let hash = aqua_ui::tree::hash_key(aqua_ui::element::Key::from(QUERY_KEY));
        app.tree()
            .sole_node_with_key(hash)
            .is_some_and(|node| self.editor.sync(app.tree_mut(), node))
    }

    pub(crate) fn dispatch(&self, palette: &mut Palette, event: Event) -> Outcome {
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
            Action::RetrySearch => Outcome {
                changed: true,
                effect: Effect::Search(palette.query.clone()),
            },
            Action::Dismiss => Outcome {
                changed: true,
                effect: Effect::Close(None),
            },
        }
    }

    pub(crate) fn content(
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
        if let Some(problem) = palette.search_error {
            let callback = guarded(self.session, &self.active, emit.clone());
            let (text, label, action) = match problem {
                SearchProblem::NoForge => (
                    "Connect to a Forge to search older threads.".to_owned(),
                    "Forges",
                    Action::Select(Screen::Settings(SettingsPage::Hosts)),
                ),
                SearchProblem::Unreachable(1) => (
                    "Search couldn't reach 1 Forge.".to_owned(),
                    "Try again",
                    Action::RetrySearch,
                ),
                SearchProblem::Unreachable(count) => (
                    format!("Search couldn't reach {count} Forges."),
                    "Try again",
                    Action::RetrySearch,
                ),
            };
            content = content.child(
                crate::ui::page::notice(theme, text)
                    .action(
                        Button::text(
                            "command-palette-search-action",
                            *theme,
                            ButtonTone::Neutral,
                            label,
                        )
                        .size(ButtonSize::Sm)
                        .on_activate(move |_| callback(action.clone()))
                        .build(),
                    )
                    .build(),
            );
        }
        if palette.searching() {
            content = content.child(caption(theme, "Searching threads…".to_owned()));
        } else if palette.entries.is_empty() && !palette.query.trim().is_empty() {
            content = content.child(caption(
                theme,
                format!("Nothing matches “{}”.", palette.query.trim()),
            ));
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
}

impl Drop for PaletteView {
    fn drop(&mut self) {
        self.close();
    }
}

fn row(theme: &ArtisanTheme, index: usize, entry: &super::Entry, selected: bool) -> RowItem {
    let row = if entry.detail.is_empty() {
        RowItem::new(
            index,
            *theme,
            Row::ListItem(Density::Regular),
            Surface::Dialog,
            entry.label.clone(),
        )
    } else {
        RowItem::new(
            index,
            *theme,
            Row::ListItem(Density::Tall),
            Surface::Dialog,
            entry.label.clone(),
        )
        .detail(entry.detail.clone())
    };
    row.selected(selected)
}

fn caption(theme: &ArtisanTheme, text: String) -> Element {
    TextRole::Caption
        .text(theme, text)
        .gpui_source_color(theme.colors.text_muted.to_reference_text_color())
        .build()
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
