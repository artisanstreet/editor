use std::time::{Duration, Instant};

use super::picker_motion::MenuMotion;

pub const OPTION_HEIGHT: f32 = match super::kit::Row::MenuItem.height() {
    Some(height) => height,
    None => 0.0,
};
pub const MENU_MIN_WIDTH: f32 = 160.0;
const MENU: super::kit::Surface = super::kit::Surface::Menu;
const FIELD: super::kit::Surface = super::kit::Surface::Field;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectEntry<V> {
    pub value: V,
    pub label: String,
    pub disabled: bool,
}

impl<V> SelectEntry<V> {
    pub fn item(value: V, label: impl Into<String>) -> Self {
        Self {
            value,
            label: label.into(),
            disabled: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SelectState<V> {
    pub open: bool,
    pub shown: bool,
    pub motion: MenuMotion,
    pub highlighted: Option<V>,
    pub keyed: bool,
    query: String,
    typed_at: Instant,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectKeyResult<V> {
    pub handled: bool,
    pub commit: Option<V>,
    pub reveal: Option<usize>,
}

impl<V: Clone + Eq> SelectState<V> {
    pub fn new(now: Instant) -> Self {
        Self {
            open: false,
            shown: false,
            motion: MenuMotion::default(),
            highlighted: None,
            keyed: false,
            query: String::new(),
            typed_at: now,
        }
    }

    pub fn reconcile(&mut self, entries: &[SelectEntry<V>], selected: Option<&V>, disabled: bool) {
        if !self.open
            || !self.highlighted.as_ref().is_some_and(|value| {
                entries
                    .iter()
                    .any(|entry| &entry.value == value && !entry.disabled)
            })
        {
            self.highlighted = selected
                .and_then(|value| {
                    entries
                        .iter()
                        .find(|entry| &entry.value == value && !entry.disabled)
                })
                .or_else(|| entries.iter().find(|entry| !entry.disabled))
                .map(|entry| entry.value.clone());
        }
        if disabled || entries.iter().all(|entry| entry.disabled) {
            self.close_now();
        }
    }

    pub fn set_open(&mut self, open: bool) -> Option<u64> {
        if self.open == open {
            return None;
        }
        self.open = open;
        let generation = if open {
            self.motion.begin_open()
        } else {
            self.motion.begin_close()
        };
        if open {
            self.shown = true;
        } else {
            self.keyed = false;
            self.query.clear();
        }
        Some(generation)
    }

    pub fn close_now(&mut self) {
        self.open = false;
        self.shown = false;
        self.keyed = false;
        self.motion.hide();
    }

    pub fn key(
        &mut self,
        key: &str,
        modified: bool,
        now: Instant,
        entries: &[SelectEntry<V>],
    ) -> SelectKeyResult<V> {
        let none = || SelectKeyResult {
            handled: false,
            commit: None,
            reveal: None,
        };
        let enabled: Vec<usize> = entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| (!entry.disabled).then_some(index))
            .collect();
        if enabled.is_empty() {
            return none();
        }
        let current = enabled
            .iter()
            .position(|index| self.highlighted.as_ref() == Some(&entries[*index].value))
            .unwrap_or(0);
        let target = match key {
            "escape" | "tab" => {
                let was_open = self.open;
                self.set_open(false);
                return SelectKeyResult {
                    handled: was_open && key != "tab",
                    commit: None,
                    reveal: None,
                };
            }
            "enter" | "space" if self.open => {
                let commit = self.highlighted.clone();
                self.set_open(false);
                return SelectKeyResult {
                    handled: true,
                    commit,
                    reveal: None,
                };
            }
            "enter" | "space" => {
                self.set_open(true);
                self.keyed = true;
                let reveal = entries
                    .iter()
                    .position(|entry| Some(&entry.value) == self.highlighted.as_ref());
                return SelectKeyResult {
                    handled: true,
                    commit: None,
                    reveal,
                };
            }
            "down" => {
                if self.open {
                    (current + 1).min(enabled.len() - 1)
                } else {
                    current
                }
            }
            "up" => {
                if self.open {
                    current.saturating_sub(1)
                } else {
                    current
                }
            }
            "home" => 0,
            "end" => enabled.len() - 1,
            key if key.chars().count() == 1 && !modified => {
                if now.duration_since(self.typed_at) > Duration::from_secs(1) {
                    self.query.clear();
                }
                self.typed_at = now;
                self.query.push_str(&key.to_lowercase());
                enabled
                    .iter()
                    .position(|index| {
                        entries[*index]
                            .label
                            .to_lowercase()
                            .starts_with(&self.query)
                    })
                    .unwrap_or(current)
            }
            _ => return none(),
        };
        self.set_open(true);
        self.keyed = true;
        let index = enabled[target];
        self.highlighted = Some(entries[index].value.clone());
        SelectKeyResult {
            handled: true,
            commit: None,
            reveal: Some(index),
        }
    }
}

use aqua_core::{InputEvent, Key as InputKey, NamedKey, Role};

use aqua_ui::{
    element::{Box as UiBox, Button, Element, Key},
    input::{EventPhase, InputKind, InputResponse, RoutedInput},
    style::{Length, Style},
};
use editor_assets::AssetId;
use std::cell::RefCell;
use std::rc::Rc;

use super::{
    chrome::account::MENU_GAP,
    kit::{IconSize, Row, RowItem, Space, TextRole, control::focus_ring},
    theme::{ArtisanTheme, Opacity},
};

type SelectChange<V> = Rc<dyn Fn(V)>;
type OpenChange = Rc<dyn Fn(bool)>;

pub struct Select<V: Clone + Eq + 'static> {
    id: Key,
    theme: ArtisanTheme,
    selected: Option<V>,
    entries: Rc<Vec<SelectEntry<V>>>,
    label: String,
    state: Rc<RefCell<SelectState<V>>>,
    disabled: bool,
    change: Option<SelectChange<V>>,
    open_change: Option<OpenChange>,
}

impl<V: Clone + Eq + 'static> Select<V> {
    pub fn new(
        id: impl Into<Key>,
        theme: ArtisanTheme,
        selected: Option<V>,
        entries: Vec<SelectEntry<V>>,
        state: Rc<RefCell<SelectState<V>>>,
    ) -> Self {
        Self {
            id: id.into(),
            theme,
            selected,
            entries: Rc::new(entries),
            label: "Choose".into(),
            state,
            disabled: false,
            change: None,
            open_change: None,
        }
    }
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }
    pub fn on_change(mut self, change: impl Fn(V) + 'static) -> Self {
        self.change = Some(Rc::new(change));
        self
    }
    pub fn on_open_change(mut self, change: impl Fn(bool) + 'static) -> Self {
        self.open_change = Some(Rc::new(change));
        self
    }

    pub fn build(self) -> Element {
        let disabled = self.disabled || self.entries.iter().all(|entry| entry.disabled);
        self.state
            .borrow_mut()
            .reconcile(&self.entries, self.selected.as_ref(), disabled);
        let open = self.state.borrow().open;
        let display = self
            .entries
            .iter()
            .find(|entry| self.selected.as_ref() == Some(&entry.value))
            .map_or_else(|| self.label.clone(), |entry| entry.label.clone());
        let theme = self.theme;
        let label = TextRole::Label
            .text(&theme, display.clone())
            .style(
                Style::new()
                    .flex_grow(1.0)
                    .min_width(Length::Px(0.0))
                    .text_wrap(false),
            )
            .build();
        let icon = IconSize::Sm.glyph(
            AssetId::TABLER_SELECTOR,
            theme.colors.text_muted.to_reference_paint(),
        );
        let state = self.state.clone();
        let open_change = self.open_change.clone();
        let tone = |open_opacity, rest_opacity| {
            theme
                .colors
                .text
                .at(if open { open_opacity } else { rest_opacity })
                .to_paint()
        };
        let mut button = Button::new()
            .width(Length::Pct(100.0))
            .height(theme.density.control_default)
            .gap(Space::S8.px())
            .padding(FIELD.padding_px())
            .radius(FIELD.radius_px())
            .border((1.0, tone(Opacity::HoverRing, Opacity::GlassBorder)))
            .fill(tone(Opacity::HoverBottom, Opacity::GlassBase))
            .style(
                Style::new()
                    .align(aqua_ui::AlignItems::CENTER)
                    .justify(aqua_ui::JustifyContent::SPACE_BETWEEN),
            )
            .child(label)
            .child(icon);
        if disabled {
            button = button.opacity(Opacity::Disabled.alpha());
        } else {
            button = button.on_activate(move || {
                let open = {
                    let mut state = state.borrow_mut();
                    let open = !state.open;
                    state.set_open(open);
                    open
                };
                if let Some(change) = &open_change {
                    change(open);
                }
            });
        }
        let mut trigger = button
            .build()
            .with_key(self.id)
            .role(Role::ComboBox)
            .aria_label(self.label)
            .aria_value(display)
            .aria_expanded(open)
            .disabled(disabled);
        if !disabled {
            trigger = trigger
                .cursor(aqua_core::CursorIcon::Pointer)
                .focus_visible_style(focus_ring(&theme));
            let state = self.state;
            let entries = self.entries;
            let change = self.change;
            let open_change = self.open_change;
            trigger = trigger.on_input(InputKind::KeyDown, EventPhase::Bubble, move |event| {
                let RoutedInput::Native(InputEvent::KeyDown(key)) = &event.input else {
                    return InputResponse::default();
                };
                let name = match key.key {
                    InputKey::Named(NamedKey::Escape) => "escape".to_owned(),
                    InputKey::Named(NamedKey::Tab) => "tab".to_owned(),
                    InputKey::Named(NamedKey::Enter) => "enter".to_owned(),
                    InputKey::Char(' ') => "space".to_owned(),
                    InputKey::Named(NamedKey::Down) => "down".to_owned(),
                    InputKey::Named(NamedKey::Up) => "up".to_owned(),
                    InputKey::Named(NamedKey::Home) => "home".to_owned(),
                    InputKey::Named(NamedKey::End) => "end".to_owned(),
                    InputKey::Char(character) => character.to_string(),
                    _ => return InputResponse::default(),
                };
                let (result, before, after) = {
                    let mut state = state.borrow_mut();
                    let before = state.open;
                    let result = state.key(
                        &name,
                        key.modifiers.contains(aqua_core::Modifiers::CONTROL)
                            || key.modifiers.contains(aqua_core::Modifiers::ALT)
                            || key.modifiers.contains(aqua_core::Modifiers::LOGO),
                        Instant::now(),
                        &entries,
                    );
                    (result, before, state.open)
                };
                if before != after {
                    if let Some(change) = &open_change {
                        change(after);
                    }
                }
                if let Some(value) = result.commit {
                    if let Some(change) = &change {
                        change(value);
                    }
                }
                if result.handled {
                    InputResponse {
                        stop_propagation: true,
                        ..InputResponse::handled()
                    }
                } else {
                    InputResponse::default()
                }
            });
        }
        trigger
    }
}

pub struct SelectMenu<V: Clone + Eq + 'static> {
    theme: ArtisanTheme,
    selected: Option<V>,
    entries: Vec<SelectEntry<V>>,
    state: Rc<RefCell<SelectState<V>>>,
    trigger_width: f32,
    change: Option<SelectChange<V>>,
    open_change: Option<OpenChange>,
}

impl<V: Clone + Eq + 'static> SelectMenu<V> {
    pub fn new(
        theme: ArtisanTheme,
        selected: Option<V>,
        entries: Vec<SelectEntry<V>>,
        state: Rc<RefCell<SelectState<V>>>,
        trigger_width: f32,
    ) -> Self {
        Self {
            theme,
            selected,
            entries,
            state,
            trigger_width,
            change: None,
            open_change: None,
        }
    }
    pub fn on_change(mut self, change: impl Fn(V) + 'static) -> Self {
        self.change = Some(Rc::new(change));
        self
    }
    pub fn on_open_change(mut self, change: impl Fn(bool) + 'static) -> Self {
        self.open_change = Some(Rc::new(change));
        self
    }

    pub fn into_parts(self) -> (Element, aqua_ui::popup::PopupOptions) {
        let state = self.state.clone();
        let open_change = self.open_change.clone();
        let mut options = aqua_ui::popup::PopupOptions::menu();
        options.kind = aqua_ui::overlay::OverlayKind::Popover;
        options.focus = aqua_ui::popup::PopupFocus::Preserve;
        options.placement.align = aqua_ui::overlay::Align::Start;
        options.placement.gap = MENU_GAP;
        options.placement.flip = false;
        options.on_close = Some(Rc::new(move |_| {
            let mut state = state.borrow_mut();
            let was_open = state.open;
            state.set_open(false);
            drop(state);
            if was_open {
                if let Some(change) = &open_change {
                    change(false);
                }
            }
        }));
        (self.build(), options)
    }

    pub fn build(self) -> Element {
        use aqua_ui::element::Scroll;
        let theme = self.theme;
        let (keyed, highlighted) = {
            let state = self.state.borrow();
            (state.keyed, state.highlighted.clone())
        };
        let count = self.entries.len();
        let check = IconSize::Md;
        let mut rows = UiBox::new().layout("column").width(Length::Pct(100.0));
        for (index, entry) in self.entries.into_iter().enumerate() {
            let chosen = self.selected.as_ref() == Some(&entry.value);
            let highlighted = keyed && highlighted.as_ref() == Some(&entry.value);
            let mark = match chosen {
                true => check.glyph(
                    AssetId::TABLER_CHECK,
                    theme.colors.text_muted.to_reference_paint(),
                ),
                false => UiBox::new().width(check.px()).height(check.px()).build(),
            };
            let mut row = RowItem::new(
                index.to_string(),
                theme,
                Row::MenuItem,
                MENU,
                entry.label.clone(),
            )
            .trailing(mark)
            .selected(highlighted)
            .disabled(entry.disabled)
            .role(Role::Option);
            if !entry.disabled {
                let state = self.state.clone();
                let change = self.change.clone();
                let open_change = self.open_change.clone();
                let value = entry.value.clone();
                let hover_state = self.state.clone();
                row = row
                    .on_activate(move || {
                        state.borrow_mut().set_open(false);
                        if let Some(change) = &open_change {
                            change(false);
                        }
                        if let Some(change) = &change {
                            change(value.clone());
                        }
                    })
                    .on_hover(move |hovered| {
                        if hovered {
                            hover_state.borrow_mut().keyed = false;
                        }
                    });
            }
            rows = rows.child(row.build().aria_selected(chosen).focusable(false));
        }
        let height =
            (OPTION_HEIGHT * count.max(1) as f32).min(theme.density.command_list_max_height);
        let scroll = Scroll::new()
            .width(Length::Pct(100.0))
            .height(height)
            .child(rows.build())
            .build();
        MENU.build(&theme, vec![scroll])
            .style(Style::new().width(Length::Px(self.trigger_width.max(MENU_MIN_WIDTH))))
            .role(Role::ListBox)
    }
}
