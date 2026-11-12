use std::rc::Rc;

use aqua_core::{
    InputEvent, Insets, Key as InputKey, NamedKey, Oklch, Role,
};
use aqua_text::{FontWeight, TextStyle};
use aqua_ui::{
    element::{Box as UiBox, Button, Element, Key, Text},
    input::{EventPhase, InputKind, InputResponse, RoutedInput},
    style::{Length, Style},
};

use super::kit::{Hover, Radius, Space, TextRole};
use super::theme::{ArtisanTheme, Opacity};

const SEGMENT_HEIGHT: f32 = 24.0;

pub struct Segment<T> {
    value: T,
    label: String,
}
impl<T> Segment<T> {
    pub fn new(value: T, label: impl Into<String>) -> Self {
        Self {
            value,
            label: label.into(),
        }
    }
}

type Change<T> = Rc<dyn Fn(T)>;

pub struct SegmentedControl<T: Clone + Eq + 'static> {
    id: Key,
    theme: ArtisanTheme,
    label: String,
    selected: T,
    options: Vec<Segment<T>>,
    disabled: bool,
    change: Option<Change<T>>,
    focus_change: Option<Rc<dyn Fn(usize)>>,
    focus_request: Option<Rc<std::cell::Cell<Option<(aqua_core::NodeId, usize)>>>>,
}

impl<T: Clone + Eq + 'static> SegmentedControl<T> {
    pub fn new(
        id: impl Into<Key>,
        theme: ArtisanTheme,
        label: impl Into<String>,
        selected: T,
        first: Segment<T>,
    ) -> Self {
        Self {
            id: id.into(),
            theme,
            label: label.into(),
            selected,
            options: vec![first],
            disabled: false,
            change: None,
            focus_change: None,
            focus_request: None,
        }
    }
    pub fn options(mut self, options: impl IntoIterator<Item = Segment<T>>) -> Self {
        for option in options {
            if !self
                .options
                .iter()
                .any(|existing| existing.value == option.value)
            {
                self.options.push(option);
            }
        }
        self
    }
    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
    pub fn on_change(mut self, handler: impl Fn(T) + 'static) -> Self {
        self.change = Some(Rc::new(handler));
        self
    }
    pub fn on_focus_change(mut self, handler: impl Fn(usize) + 'static) -> Self {
        self.focus_change = Some(Rc::new(handler));
        self
    }

    pub fn focus_host(mut self, host: &SegmentedFocus) -> Self {
        self.focus_request = Some(host.pending.clone());
        self
    }

    pub fn build(self) -> Element {
        let theme = self.theme;
        let selected = self
            .options
            .iter()
            .position(|option| option.value == self.selected)
            .unwrap_or(0);
        let count = self.options.len();
        let values = Rc::new(
            self.options
                .iter()
                .map(|option| option.value.clone())
                .collect::<Vec<_>>(),
        );
        let mut root = UiBox::new()
            .gap(Space::S2.px())
            .style(Style::new().align(aqua_ui::AlignItems::CENTER));
        for (index, option) in self.options.into_iter().enumerate() {
            let chosen = index == selected;
            let foreground = if chosen {
                theme.colors.text
            } else {
                theme.colors.text_muted
            };
            let font = TextStyle {
                weight: if chosen {
                    FontWeight(600)
                } else {
                    FontWeight::NORMAL
                },
                ..TextRole::Caption.style(&theme)
            };
            let text = Text::new(option.label.clone())
                .style(
                    Style::new()
                        .text_style(font)
                        .gpui_source_text_color(foreground.to_reference_text_color())
                        .text_wrap(false),
                )
                .build();
            let mut style = Style::new()
                .align(aqua_ui::AlignItems::CENTER)
                .justify(aqua_ui::JustifyContent::CENTER)
                .padding_insets(Insets {
                    left: Space::S8.px(),
                    right: Space::S8.px(),
                    top: 0.0,
                    bottom: 0.0,
                })
                .border_of(1.0, Oklch::TRANSPARENT);
            style = style.radius(Length::Px(Radius::Field.px()));
            if chosen {
                style.merge(&Hover::Neutral.style(&theme));
            }
            let mut button = Button::new()
                .height(SEGMENT_HEIGHT)
                .style(style)
                .child(text);
            if self.disabled {
                button = button.opacity(Opacity::Disabled.alpha());
            }
            if !self.disabled {
                if let Some(change) = self.change.clone() {
                    let value = option.value.clone();
                    button = button.on_activate(move || change(value.clone()));
                }
            }
            let mut item = button
                .build()
                .with_key(index)
                .role(Role::RadioButton)
                .aria_label(option.label)
                .aria_checked(chosen)
                .disabled(self.disabled);
            if !self.disabled {
                item = item
                    .cursor(aqua_core::CursorIcon::Pointer)
                    .tab_index(if chosen { 0 } else { -1 })
                    .hover_style(Hover::Neutral.style(&theme))
                    .focus_visible_style(Style::new().border_of(1.0, theme.colors.ring.to_paint()));
                let values = values.clone();
                let change = self.change.clone();
                let focus_change = self.focus_change.clone();
                let focus_request = self.focus_request.clone();
                item = item.on_input(InputKind::KeyDown, EventPhase::Bubble, move |event| {
                    let RoutedInput::Native(InputEvent::KeyDown(key)) = &event.input else {
                        return InputResponse::default();
                    };
                    let next = match key.key {
                        InputKey::Named(NamedKey::Left | NamedKey::Up) => {
                            if index == 0 {
                                count - 1
                            } else {
                                index - 1
                            }
                        }
                        InputKey::Named(NamedKey::Right | NamedKey::Down) => (index + 1) % count,
                        InputKey::Named(NamedKey::Home) => 0,
                        InputKey::Named(NamedKey::End) => count - 1,
                        _ => return InputResponse::default(),
                    };
                    if let Some(change) = &change {
                        change(values[next].clone());
                    }
                    if let Some(focus) = &focus_change {
                        focus(next);
                    }
                    if let Some(request) = &focus_request {
                        request.set(Some((event.current_target, next)));
                    }
                    InputResponse {
                        stop_propagation: true,
                        ..InputResponse::handled()
                    }
                });
            }
            root = root.child(item);
        }
        root.build()
            .with_key(self.id)
            .role(Role::RadioGroup)
            .aria_label(self.label)
    }
}

#[derive(Default, Clone)]
pub struct SegmentedFocus {
    pending: Rc<std::cell::Cell<Option<(aqua_core::NodeId, usize)>>>,
}

impl SegmentedFocus {
    pub fn sync(&self, app: &mut aqua::UiApp) -> bool {
        let Some((current, index)) = self.pending.take() else {
            return false;
        };
        let target = app
            .tree()
            .get(current)
            .and_then(|item| item.parent)
            .and_then(|parent| app.tree().get(parent))
            .and_then(|group| group.children.get(index))
            .copied();
        target.is_some_and(|node| app.tree_mut().set_focused(Some(node)))
    }

    pub fn handle_input(&self, app: &mut aqua::UiApp, event: &aqua_core::InputEvent) -> bool {
        let handled = app.handle_input(event);
        self.sync(app);
        handled
    }
}
