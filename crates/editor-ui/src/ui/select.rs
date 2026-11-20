use std::time::{Duration, Instant};

use super::picker_motion::MenuMotion;

const OPTION_HEIGHT: f32 = match Row::MenuItem.height() {
    Some(height) => height,
    None => 0.0,
};
const MENU_MIN_WIDTH: f32 = 160.0;
const MENU: super::kit::Surface = super::kit::Surface::Menu;
const FIELD: super::kit::Surface = super::kit::Surface::Field;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectEntry<V> {
    value: V,
    label: String,
    disabled: bool,
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
    open: bool,
    shown: bool,
    motion: MenuMotion,
    highlighted: Option<V>,
    keyed: bool,
    query: String,
    typed_at: Instant,
    scroll: MenuScroll,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum MenuScroll {
    Shown(f32),
    Reveal(f32),
}

impl MenuScroll {
    const fn offset(self) -> f32 {
        match self {
            Self::Shown(offset) | Self::Reveal(offset) => offset,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectKeyResult<V> {
    handled: bool,
    commit: Option<V>,
    reveal: Option<usize>,
}

impl<V: Clone + Eq> SelectState<V> {
    pub const fn is_open(&self) -> bool {
        self.open
    }

    pub const fn highlighted(&self) -> Option<&V> {
        self.highlighted.as_ref()
    }

    pub fn set_highlighted(&mut self, value: Option<V>) {
        self.highlighted = value;
    }

    pub fn new(now: Instant) -> Self {
        Self {
            open: false,
            shown: false,
            motion: MenuMotion::default(),
            highlighted: None,
            keyed: false,
            query: String::new(),
            typed_at: now,
            scroll: MenuScroll::Shown(0.0),
        }
    }

    fn reconcile(&mut self, entries: &[SelectEntry<V>], selected: Option<&V>, disabled: bool) {
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

    fn close_now(&mut self) {
        self.open = false;
        self.shown = false;
        self.keyed = false;
        self.motion.hide();
    }

    fn key(&mut self, key: &str, entries: &[SelectEntry<V>]) -> SelectKeyResult<V> {
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
            _ => return none(),
        };
        self.highlight(entries, enabled[target])
    }

    fn type_text(
        &mut self,
        text: &str,
        now: Instant,
        entries: &[SelectEntry<V>],
    ) -> SelectKeyResult<V> {
        let typed = typeahead_form(text);
        if text.chars().any(char::is_control) || typed.trim().is_empty() && self.query.is_empty() {
            return SelectKeyResult {
                handled: false,
                commit: None,
                reveal: None,
            };
        }
        if now.duration_since(self.typed_at) > Duration::from_secs(1) {
            self.query.clear();
        }
        self.typed_at = now;
        self.query.push_str(&typed);
        let enabled = entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| !entry.disabled);
        let matched = enabled
            .clone()
            .find(|(_, entry)| typeahead_form(&entry.label).starts_with(&self.query))
            .or_else(|| {
                enabled
                    .clone()
                    .find(|(_, entry)| Some(&entry.value) == self.highlighted.as_ref())
            })
            .or_else(|| enabled.clone().next());
        match matched {
            Some((index, _)) => self.highlight(entries, index),
            None => SelectKeyResult {
                handled: false,
                commit: None,
                reveal: None,
            },
        }
    }

    fn reveal(&mut self, index: usize, viewport: f32) {
        #[expect(
            clippy::cast_precision_loss,
            reason = "option indices are far below 2^23"
        )]
        let top = OPTION_HEIGHT * index as f32;
        let bottom = top + OPTION_HEIGHT;
        let shown = self.scroll.offset();
        if top < shown {
            self.scroll = MenuScroll::Reveal(top);
        } else if bottom > shown + viewport {
            self.scroll = MenuScroll::Reveal(bottom - viewport);
        }
    }

    fn highlight(&mut self, entries: &[SelectEntry<V>], index: usize) -> SelectKeyResult<V> {
        self.set_open(true);
        self.keyed = true;
        self.highlighted = Some(entries[index].value.clone());
        SelectKeyResult {
            handled: true,
            commit: None,
            reveal: Some(index),
        }
    }
}

fn select_key_name(key: InputKey) -> Option<&'static str> {
    match key {
        InputKey::Named(NamedKey::Escape) => Some("escape"),
        InputKey::Named(NamedKey::Tab) => Some("tab"),
        InputKey::Named(NamedKey::Enter) => Some("enter"),
        InputKey::Char(' ') => Some("space"),
        InputKey::Named(NamedKey::Down) => Some("down"),
        InputKey::Named(NamedKey::Up) => Some("up"),
        InputKey::Named(NamedKey::Home) => Some("home"),
        InputKey::Named(NamedKey::End) => Some("end"),
        InputKey::Char(_) | InputKey::Named(_) => None,
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "option counts are far below 2^23"
)]
fn menu_height(theme: &ArtisanTheme, count: usize) -> f32 {
    (OPTION_HEIGHT * count.max(1) as f32).min(theme.density.command_list_max_height)
}

fn typeahead_form(text: &str) -> String {
    icu_normalizer::ComposingNormalizerBorrowed::new_nfc()
        .normalize(&text.to_lowercase())
        .into_owned()
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
    entries: Rc<[SelectEntry<V>]>,
    label: String,
    state: Rc<RefCell<SelectState<V>>>,
    disabled: bool,
    change: Option<SelectChange<V>>,
    open_change: Option<OpenChange>,
}

impl<V: Clone + Eq + 'static> std::fmt::Debug for Select<V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Select").finish_non_exhaustive()
    }
}

impl<V: Clone + Eq + 'static> Select<V> {
    #[expect(
        clippy::large_types_passed_by_value,
        reason = "the select stores this theme copy and paints from it when built"
    )]
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
            entries: Rc::from(entries),
            label: "Choose".into(),
            state,
            disabled: false,
            change: None,
            open_change: None,
        }
    }
    #[must_use]
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }
    #[must_use]
    pub fn on_change(mut self, change: impl Fn(V) + 'static) -> Self {
        self.change = Some(Rc::new(change));
        self
    }
    #[must_use]
    pub fn on_open_change(mut self, change: impl Fn(bool) + 'static) -> Self {
        self.open_change = Some(Rc::new(change));
        self
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one select build: trigger, menu and keyboard handling"
    )]
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
        let state = Rc::clone(&self.state);
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
            for kind in [InputKind::KeyDown, InputKind::TextInput, InputKind::Ime] {
                let state = Rc::clone(&state);
                let entries = Rc::clone(&entries);
                let change = change.clone();
                let open_change = open_change.clone();
                trigger = trigger.on_input(kind, EventPhase::Bubble, move |event| {
                    let now = Instant::now();
                    let (result, before, after) = {
                        let mut state = state.borrow_mut();
                        let before = state.open;
                        let result = match &event.input {
                            RoutedInput::Native(InputEvent::KeyDown(key)) => {
                                match select_key_name(key.key) {
                                    Some(name) => state.key(name, &entries),
                                    None => return InputResponse::default(),
                                }
                            }
                            RoutedInput::Native(
                                InputEvent::TextInput { text }
                                | InputEvent::Ime(aqua_core::ImeEvent::Commit { text }),
                            ) => state.type_text(text, now, &entries),
                            RoutedInput::ClipboardPaste(_)
                            | RoutedInput::ClipboardCut
                            | RoutedInput::TextVertical { .. }
                            | RoutedInput::TextMotion { .. }
                            | RoutedInput::Native(_)
                            | RoutedInput::PointerEnter(_)
                            | RoutedInput::PointerLeave(_)
                            | RoutedInput::TextPointer { .. }
                            | RoutedInput::TextContext { .. }
                            | RoutedInput::TextRangeActivated { .. }
                            | RoutedInput::FilesDropped { .. }
                            | RoutedInput::DragEnter { .. }
                            | RoutedInput::DragOver { .. }
                            | RoutedInput::DragLeave { .. }
                            | RoutedInput::Drop { .. }
                            | RoutedInput::DragEnd { .. } => return InputResponse::default(),
                        };
                        if let Some(index) = result.reveal {
                            state.reveal(index, menu_height(&theme, entries.len()));
                        }
                        (result, before, state.open)
                    };
                    if before != after
                        && let Some(change) = &open_change
                    {
                        change(after);
                    }
                    if let Some(value) = result.commit
                        && let Some(change) = &change
                    {
                        change(value);
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

impl<V: Clone + Eq + 'static> std::fmt::Debug for SelectMenu<V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SelectMenu").finish_non_exhaustive()
    }
}

impl<V: Clone + Eq + 'static> SelectMenu<V> {
    #[expect(
        clippy::large_types_passed_by_value,
        reason = "the select menu stores this theme copy and paints from it when built"
    )]
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
    #[must_use]
    pub fn on_change(mut self, change: impl Fn(V) + 'static) -> Self {
        self.change = Some(Rc::new(change));
        self
    }
    #[must_use]
    pub fn on_open_change(mut self, change: impl Fn(bool) + 'static) -> Self {
        self.open_change = Some(Rc::new(change));
        self
    }

    pub fn into_parts(self) -> (Element, aqua_ui::popup::PopupOptions) {
        let state = Rc::clone(&self.state);
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
            if was_open && let Some(change) = &open_change {
                change(false);
            }
        }));
        (self.build(), options)
    }

    fn build(self) -> Element {
        use aqua_ui::element::Scroll;
        let theme = self.theme;
        let (keyed, highlighted, scroll) = {
            let mut state = self.state.borrow_mut();
            let scroll = state.scroll;
            state.scroll = MenuScroll::Shown(scroll.offset());
            (state.keyed, state.highlighted.clone(), scroll)
        };
        let count = self.entries.len();
        let check = IconSize::Md;
        let mut rows = UiBox::new().layout("column").width(Length::Pct(100.0));
        for (index, entry) in self.entries.into_iter().enumerate() {
            let chosen = self.selected.as_ref() == Some(&entry.value);
            let highlighted = keyed && highlighted.as_ref() == Some(&entry.value);
            let mark = if chosen {
                check.glyph(
                    AssetId::TABLER_CHECK,
                    theme.colors.text_muted.to_reference_paint(),
                )
            } else {
                UiBox::new().width(check.px()).height(check.px()).build()
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
                let state = Rc::clone(&self.state);
                let change = self.change.clone();
                let open_change = self.open_change.clone();
                let value = entry.value.clone();
                let hover_state = Rc::clone(&self.state);
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
            let row = row.build().aria_selected(chosen).focusable(false);
            rows = rows.child(if highlighted {
                row.aria_active_descendant()
            } else {
                row
            });
        }
        let scroll = Scroll::new()
            .layout("column")
            .width(Length::Pct(100.0))
            .height(menu_height(&theme, count))
            .offset(aqua_core::Point::new(0.0, scroll.offset()))
            .preserve_offset(matches!(scroll, MenuScroll::Shown(_)))
            .child(rows.build())
            .build();
        let shown = Rc::clone(&self.state);
        let reader = aqua_ui::element::Canvas::new()
            .style(Style::new().absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0)))
            .on_prepaint(move |tree, node| {
                let offset = tree
                    .get(node)
                    .and_then(|probe| probe.parent)
                    .and_then(|surface| tree.get(surface))
                    .and_then(|surface| {
                        surface
                            .children
                            .iter()
                            .find_map(|child| tree.scroll_offset(*child))
                    });
                let mut state = shown.borrow_mut();
                if let (Some(offset), MenuScroll::Shown(_)) = (offset, state.scroll) {
                    state.scroll = MenuScroll::Shown(offset.y);
                }
            })
            .build()
            .exclude_from_hit_testing();
        MENU.build(&theme, vec![scroll, reader])
            .style(Style::new().width(Length::Px(self.trigger_width.max(MENU_MIN_WIDTH))))
            .role(Role::ListBox)
    }
}

#[cfg(test)]
#[path = "../../tests/ui/select.rs"]
mod tests;
