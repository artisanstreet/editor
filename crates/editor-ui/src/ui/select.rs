use std::time::{Duration, Instant};

use super::picker_motion::MenuMotion;

pub const MENU_MOTION: Duration = super::picker_motion::DROPDOWN_OPEN;
pub const OPTION_HEIGHT: f32 = match super::kit::Row::MenuItem.height() {
    Some(height) => height,
    None => 0.0,
};
pub const MENU_MIN_WIDTH: f32 = 160.0;
const ROW_RADIUS: f32 = super::kit::Radius::Row.px();

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

    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
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

    pub fn finish(&mut self, generation: u64, open: bool) -> bool {
        let settled = if open {
            self.motion.finish_open(generation)
        } else {
            self.motion.finish_close(generation)
        };
        if settled && !open {
            self.shown = false;
        }
        settled
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
    element::{Button, Element, Key, Text},
    input::{EventPhase, InputKind, InputResponse, RoutedInput},
    style::{Length, Style},
};
use editor_assets::AssetId;
use std::cell::RefCell;
use std::rc::Rc;

use super::{
    asset::source_asset_glyph,
    kit::{Hover, IconSize, Radius, Row, Space, Surface, TextRole},
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
    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
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
        let text_style = TextRole::Label.style(&theme);
        let label = Text::new(display.clone())
            .style(
                Style::new()
                    .flex_grow(1.0)
                    .min_width(Length::Px(0.0))
                    .text_style(text_style)
                    .gpui_source_text_color(theme.colors.text.to_reference_text_color())
                    .text_wrap(false),
            )
            .build();
        let icon = source_asset_glyph(
            AssetId::TABLER_SELECTOR,
            IconSize::Sm.px(),
            theme.colors.text_muted.to_reference_paint(),
        );
        let state = self.state.clone();
        let open_change = self.open_change.clone();
        let mut button = Button::new()
            .width(Length::Pct(100.0))
            .height(theme.density.control_default)
            .gap(Space::S8.px())
            .radius(Radius::Field.px())
            .border((
                1.0,
                theme
                    .colors
                    .text
                    .at(if open {
                        Opacity::HoverRing
                    } else {
                        Opacity::GlassBorder
                    })
                    .to_paint(),
            ))
            .fill(
                theme
                    .colors
                    .text
                    .at(if open {
                        Opacity::HoverBottom
                    } else {
                        Opacity::GlassBase
                    })
                    .to_paint(),
            )
            .style(
                Style::new()
                    .padding_insets(aqua_core::Insets {
                        left: Space::S12.px(),
                        right: Space::S12.px(),
                        top: 0.0,
                        bottom: 0.0,
                    })
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
                .focus_visible_style(
                    Style::new()
                        .gpui_border(
                            aqua_core::Insets::uniform(1.0),
                            theme.colors.ring.to_reference_paint(),
                            false,
                        )
                        .gpui_shadows(vec![aqua_ui::style::GpuiShadowLayer {
                            dx: 0.0,
                            dy: 0.0,
                            blur: 0.0,
                            spread: theme.interaction.focus_ring_width,
                            inset: false,
                            color: theme
                                .colors
                                .ring
                                .at(Opacity::FocusRing)
                                .to_reference_paint(),
                        }]),
                );
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
    hover: Rc<RefCell<super::picker_motion::HoverSurface>>,
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
            hover: Rc::new(RefCell::new(super::picker_motion::HoverSurface::new(
                "select-hover",
            ))),
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
        options.placement.gap = 6.0;
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
        use aqua_ui::element::{Box as UiBox, Scroll};
        let theme = self.theme;
        let (keyed, highlighted) = {
            let state = self.state.borrow();
            (state.keyed, state.highlighted.clone())
        };
        let count = self.entries.len();
        let text_style = TextRole::Label.style(&theme);
        let pad = Row::MenuItem.pad();
        let check = IconSize::Md.px();
        let mut rows = UiBox::new().layout("column").width(Length::Pct(100.0));
        for (index, entry) in self.entries.into_iter().enumerate() {
            let chosen = self.selected.as_ref() == Some(&entry.value);
            let highlighted = keyed && highlighted.as_ref() == Some(&entry.value);
            let text = Text::new(entry.label.clone())
                .style(
                    Style::new()
                        .flex_grow(1.0)
                        .min_width(Length::Px(0.0))
                        .text_style(text_style.clone())
                        .gpui_source_text_color(theme.colors.text.to_reference_text_color())
                        .text_wrap(false),
                )
                .build();
            let mut row = Button::new()
                .height(OPTION_HEIGHT)
                .width(Length::Pct(100.0))
                .radius(ROW_RADIUS)
                .style(Style::new().padding_insets(aqua_core::Insets {
                    left: pad.x.px(),
                    right: 2.0 * pad.x.px() + check,
                    top: pad.y.px(),
                    bottom: pad.y.px(),
                }))
                .child(text);
            if highlighted {
                row = row.style(Hover::Neutral.style(&theme));
            }
            if entry.disabled {
                row = row.opacity(Opacity::Disabled.alpha());
            }
            if chosen {
                let icon = source_asset_glyph(
                    AssetId::TABLER_CHECK,
                    check,
                    theme.colors.text_muted.to_reference_paint(),
                )
                .style(Style::new().absolute_at(
                    None,
                    Some(pad.y.px() + (Row::MenuItem.lines() - check) / 2.0),
                    Some(pad.x.px()),
                    None,
                ));
                row = row.child(icon);
            }
            if !entry.disabled {
                let state = self.state.clone();
                let change = self.change.clone();
                let open_change = self.open_change.clone();
                let value = entry.value.clone();
                row = row.on_activate(move || {
                    state.borrow_mut().set_open(false);
                    if let Some(change) = &open_change {
                        change(false);
                    }
                    if let Some(change) = &change {
                        change(value.clone());
                    }
                });
            }
            let mut row = row
                .build()
                .with_key(index.to_string())
                .role(Role::Option)
                .aria_label(entry.label)
                .aria_selected(chosen)
                .disabled(entry.disabled)
                .focusable(false);
            if !entry.disabled {
                let hover = self.hover.clone();
                let state = self.state.clone();
                let id = index.to_string();
                row = row
                    .cursor(aqua_core::CursorIcon::Pointer)
                    .on_hover_change(move |hovered| {
                        hover
                            .borrow()
                            .hovered(&id, hovered, super::hover::HoverTone::Neutral);
                        if hovered {
                            state.borrow_mut().keyed = false;
                        }
                    });
            }
            rows = rows.child(row);
        }
        let height =
            (OPTION_HEIGHT * count.max(1) as f32).min(theme.density.command_list_max_height);
        let scroll = Scroll::new()
            .width(Length::Pct(100.0))
            .height(height)
            .child(rows.build())
            .build();
        Surface::Menu
            .build(
                &theme,
                vec![self.hover.borrow().pill(&theme, ROW_RADIUS), scroll],
            )
            .style(Style::new().width(Length::Px(self.trigger_width.max(MENU_MIN_WIDTH))))
            .role(Role::ListBox)
    }
}

pub struct SelectPopupHost<V: Clone + Eq + 'static> {
    overlay: aqua_ui::overlay::OverlayId,
    state: Rc<RefCell<SelectState<V>>>,
    factory: Rc<dyn Fn() -> Element>,
    on_open_change: Option<OpenChange>,
    flight: Option<(u64, bool, f64)>,
    scale: f32,
    from_scale: f32,
    now: f64,
    hover: Rc<RefCell<super::picker_motion::HoverSurface>>,
    theme: ArtisanTheme,
    hover_changed: bool,
}

impl<V: Clone + Eq + 'static> SelectPopupHost<V> {
    pub fn open(
        app: &mut aqua::UiApp,
        owner: aqua_core::NodeId,
        menu: SelectMenu<V>,
        now: f64,
    ) -> Result<Self, String> {
        use aqua_ui::overlay::{Align, DismissPolicy, OverlayKind};
        use aqua_ui::popup::{PopupFocus, PopupOptions};
        let state = menu.state.clone();
        if !state.borrow().open {
            state.borrow_mut().set_open(true);
        }
        let on_open_change = menu.open_change.clone();
        let hover = menu.hover.clone();
        let theme = menu.theme;
        let factory: Rc<dyn Fn() -> Element> = Rc::new(move || {
            SelectMenu {
                theme: menu.theme,
                selected: menu.selected.clone(),
                entries: menu.entries.clone(),
                state: menu.state.clone(),
                trigger_width: menu.trigger_width,
                change: menu.change.clone(),
                open_change: menu.open_change.clone(),
                hover: menu.hover.clone(),
            }
            .build()
        });
        let mut options = PopupOptions::menu();
        options.kind = OverlayKind::Popover;
        options.focus = PopupFocus::Preserve;
        options.dismiss = DismissPolicy::MANUAL;
        options.placement.align = Align::Start;
        options.placement.gap = 6.0;
        options.placement.flip = false;
        let overlay = app
            .open_popup_measured(owner, factory(), options)
            .map_err(|error| error.to_string())?;
        app.set_popup_uncontained(overlay, true);
        let mut host = Self {
            overlay,
            state,
            factory,
            on_open_change,
            flight: None,
            scale: super::picker_motion::DROPDOWN_PRE_SCALE,
            from_scale: super::picker_motion::DROPDOWN_PRE_SCALE,
            now,
            hover,
            theme,
            hover_changed: false,
        };
        host.advance(app, now)?;
        Ok(host)
    }

    pub fn advance(&mut self, app: &mut aqua::UiApp, now: f64) -> Result<(), String> {
        use super::picker_motion::MenuPhase;
        self.now = now;
        let (phase, transition) = {
            let state = self.state.borrow();
            (state.motion.phase(), state.motion.transition())
        };
        let Some((_, _, _, _, generation)) = transition else {
            if phase == MenuPhase::Open {
                app.refresh_popup(self.overlay, (self.factory)())
                    .map_err(|error| error.to_string())?;
            }
            self.advance_hover(app, now)?;
            return Ok(());
        };
        let closing = phase == MenuPhase::Closing;
        if self
            .flight
            .as_ref()
            .is_none_or(|(old, _, _)| *old != generation)
        {
            self.from_scale = self.scale;
            self.flight = Some((generation, closing, now));
        }
        let Some((_, _, started)) = self.flight else {
            return Ok(());
        };
        let duration = if closing {
            super::picker_motion::DROPDOWN_CLOSE
        } else {
            MENU_MOTION
        }
        .as_secs_f64();
        let reduced =
            app.host().motion().reduced_motion() == aqua_ui::motion::ReducedMotion::Reduce;
        let progress = if duration > 0.0 && !reduced {
            ((now - started) / duration).clamp(0.0, 1.0) as f32
        } else {
            1.0
        };
        let settled = progress >= 1.0;
        let (opacity, _) = {
            let mut state = self.state.borrow_mut();
            state
                .motion
                .apply_progress(generation, super::picker_motion::smooth_out(progress));
            let frame = state.motion.frame();
            if settled {
                state.finish(generation, !closing);
            }
            frame
        };
        let target_scale = if closing {
            super::picker_motion::DROPDOWN_CLOSING_SCALE
        } else {
            1.0
        };
        self.scale = (target_scale - self.from_scale)
            .mul_add(super::picker_motion::smooth_out(progress), self.from_scale);
        let scale = self.scale;
        let element = (self.factory)().style(
            Style::new()
                .opacity(opacity)
                .transform(aqua_core::Transform2D::scale(scale, scale)),
        );
        app.refresh_popup(self.overlay, element)
            .map_err(|error| error.to_string())?;
        self.advance_hover(app, now)?;
        if settled {
            self.flight = None;
            if closing {
                self.hover.borrow_mut().dispose(app);
                app.close_popup(self.overlay);
            }
        }
        Ok(())
    }

    fn advance_hover(&mut self, app: &mut aqua::UiApp, now: f64) -> Result<(), String> {
        if let Some(root) = app.host().popup_root(self.overlay) {
            let reduced =
                app.host().motion().reduced_motion() == aqua_ui::motion::ReducedMotion::Reduce;
            let frame = self.hover.borrow_mut().advance_frame(
                app,
                root,
                &self.theme,
                now,
                reduced,
            )?;
            self.hover_changed |= frame.changed;
        }
        Ok(())
    }

    pub fn handle_input(
        &mut self,
        app: &mut aqua::UiApp,
        event: &aqua_core::InputEvent,
    ) -> Result<bool, String> {
        if let aqua_core::InputEvent::PointerDown(pointer, aqua_core::PointerButton::Primary) =
            event
        {
            let outside = app
                .host()
                .service()
                .bounds(self.overlay)
                .is_some_and(|bounds| !bounds.contains(pointer.position));
            if outside && self.state.borrow().open {
                self.state.borrow_mut().set_open(false);
                if let Some(change) = &self.on_open_change {
                    change(false);
                }
            }
        }
        let handled = app.handle_input(event);
        self.advance(app, self.now)?;
        Ok(handled)
    }

    pub fn step(
        &mut self,
        app: &mut aqua::UiApp,
        input: aqua::RuntimeInput,
    ) -> Result<aqua::RuntimeOutput, String> {
        if let aqua::RuntimeInput::Shell(aqua_protocol::ShellEvent::Input {
            event: aqua_core::InputEvent::PointerDown(pointer, aqua_core::PointerButton::Primary),
            ..
        }) = &input
        {
            let outside = app
                .host()
                .service()
                .bounds(self.overlay)
                .is_some_and(|bounds| !bounds.contains(pointer.position));
            if outside && self.state.borrow().open {
                self.state.borrow_mut().set_open(false);
                if let Some(change) = &self.on_open_change {
                    change(false);
                }
            }
        }
        if let aqua::RuntimeInput::Shell(aqua_protocol::ShellEvent::Frame {
            presentation: now,
            ..
        })
        | aqua::RuntimeInput::Tick(now) = &input
        {
            self.advance(app, now.as_secs_f64())?;
        }
        app.drive(input);
        let mut output = app.take_pending();
        if self.is_open(app) {
            self.advance(app, self.now)?;
        }
        if self.take_hover_changed() {
            output.wake_at = Some(aqua::Instant::from_secs_f64(self.now));
        }
        Ok(output)
    }

    pub fn take_hover_changed(&mut self) -> bool {
        std::mem::take(&mut self.hover_changed)
    }

    pub fn is_open(&self, app: &aqua::UiApp) -> bool {
        app.host().is_open(self.overlay)
    }

    pub fn is_animating(&self, _app: &aqua::UiApp) -> bool {
        self.flight.is_some() || self.hover.borrow().is_animating()
    }
}
