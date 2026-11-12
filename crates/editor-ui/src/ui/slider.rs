#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SliderRange {
    min: f64,
    max: f64,
    step: f64,
}
impl SliderRange {
    pub fn new(min: f64, max: f64, step: Option<f64>) -> Self {
        let (min, max) = if min.is_finite() && max.is_finite() && (max - min).is_finite() {
            (min.min(max), min.max(max))
        } else {
            (0.0, 1.0)
        };
        let span = max - min;
        let fallback = if span > 0.0 {
            (span / 100.0).max(f64::MIN_POSITIVE.min(span))
        } else {
            1.0
        };
        let step = step
            .filter(|step| step.is_finite() && *step > 0.0)
            .unwrap_or(fallback);
        Self { min, max, step }
    }
    #[expect(
        clippy::float_cmp,
        reason = "the clamped ends are kept exactly, not snapped"
    )]
    pub fn snap(self, value: f64) -> f64 {
        let value = if value.is_finite() {
            value.clamp(self.min, self.max)
        } else {
            self.min
        };
        if value == self.min || value == self.max {
            return value;
        }
        let steps = ((value - self.min) / self.step).round();
        let snapped = steps.mul_add(self.step, self.min);
        if snapped.is_finite() {
            snapped.clamp(self.min, self.max)
        } else {
            value
        }
    }
    pub fn command(self, value: f64, command: SliderCommand) -> f64 {
        match command {
            SliderCommand::Start => self.min,
            SliderCommand::End => self.max,
            SliderCommand::Previous => self.snap((value - self.step).max(self.min)),
            SliderCommand::Next => self.snap((value + self.step).min(self.max)),
        }
    }
}
#[derive(Clone, Copy)]
pub enum SliderCommand {
    Start,
    End,
    Previous,
    Next,
}
impl SliderCommand {
    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "home" => Some(Self::Start),
            "end" => Some(Self::End),
            "left" | "down" => Some(Self::Previous),
            "right" | "up" => Some(Self::Next),
            _ => None,
        }
    }
}

use std::cell::RefCell;
use std::rc::Rc;

use aqua_core::{
    CornerRadii, DisplayItem, GpuiBackground, InputEvent, Insets, Key as InputKey, NamedKey,
    Oklch as PaintColor, PointerButton, Rect, Role,
};
use aqua_text::{FontFamily, FontWeight, TextStyle};
use aqua_ui::{
    element::{Box as UiBox, Canvas, Element, Key, Text},
    input::{CaptureRequest, EventPhase, FocusRequest, InputKind, InputResponse, RoutedInput},
    style::{LayoutStyle, Length, Style},
    tree::UiCommand,
};

use super::theme::{ArtisanTheme, Opacity};

pub struct SliderState {
    range: SliderRange,
    value: f64,
    dragging: bool,
}

impl SliderState {
    pub fn new(range: SliderRange, value: f64) -> Self {
        Self {
            range,
            value: range.snap(value),
            dragging: false,
        }
    }

    pub fn value(&self) -> f64 {
        self.value
    }

    pub fn reset(&mut self, range: SliderRange, value: f64) {
        self.range = range;
        self.value = range.snap(value);
        self.dragging = false;
    }

    fn set(&mut self, value: f64) -> bool {
        let value = self.range.snap(value);
        if self.value == value {
            return false;
        }
        self.value = value;
        true
    }

    fn seek(&mut self, x: f32, bounds: Rect) -> bool {
        if bounds.w <= 0.0 {
            return false;
        }
        let t = ((x - bounds.x) / bounds.w).clamp(0.0, 1.0);
        self.set(self.range.min + f64::from(t) * (self.range.max - self.range.min))
    }

    fn fraction(&self) -> f32 {
        let span = self.range.max - self.range.min;
        if span == 0.0 {
            0.0
        } else {
            ((self.value - self.range.min) / span).clamp(0.0, 1.0) as f32
        }
    }
}

pub struct Slider {
    id: Key,
    theme: ArtisanTheme,
    label: String,
    readout: String,
    state: Rc<RefCell<SliderState>>,
    disabled: bool,
    on_change: Option<Rc<dyn Fn(f64)>>,
}

impl Slider {
    pub fn new(
        id: impl Into<Key>,
        theme: ArtisanTheme,
        label: impl Into<String>,
        readout: impl Into<String>,
        state: Rc<RefCell<SliderState>>,
    ) -> Self {
        Self {
            id: id.into(),
            theme,
            label: label.into(),
            readout: readout.into(),
            state,
            disabled: false,
            on_change: None,
        }
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
    pub fn on_change(mut self, handler: impl Fn(f64) + 'static) -> Self {
        self.on_change = Some(Rc::new(handler));
        self
    }

    pub fn build(self) -> Element {
        let theme = self.theme;
        let accent = theme.colors.amber.solid.to_reference_paint();
        let state = self.state.clone();
        let layout = LayoutStyle {
            height: Length::Px(20.0),
            min_width: Length::Px(80.0),
            flex_grow: 1.0,
            ..LayoutStyle::default()
        };
        let mut track = Canvas::new()
            .style(layout.clone())
            .radius(super::kit::Radius::Field.px())
            .border((1.0, PaintColor::TRANSPARENT))
            .on_paint(move |bounds| {
                let fraction = state.borrow().fraction();
                let bar = |x: f32, width: f32, fill| DisplayItem::GpuiBackground {
                    bounds: Rect {
                        x,
                        y: bounds.y + 9.0,
                        w: width,
                        h: 2.0,
                    },
                    corners: CornerRadii::uniform(1.0),
                    background: GpuiBackground::Solid(fill),
                };
                let thumb_bounds = Rect {
                    x: bounds.x + bounds.w * fraction - 6.0,
                    y: bounds.y + 4.0,
                    w: 12.0,
                    h: 12.0,
                };
                vec![
                    bar(bounds.x, bounds.w, theme.colors.line.to_reference_paint()),
                    bar(bounds.x, bounds.w * fraction, accent),
                    DisplayItem::GpuiBackground {
                        bounds: thumb_bounds,
                        corners: CornerRadii::uniform(6.0),
                        background: GpuiBackground::Solid(
                            theme.colors.background.to_reference_paint(),
                        ),
                    },
                    DisplayItem::GpuiBorder {
                        bounds: thumb_bounds,
                        corners: CornerRadii::uniform(6.0),
                        widths: Insets::uniform(2.0),
                        color: accent,
                        dashed: false,
                    },
                ]
            })
            .build()
            .with_key(self.id)
            .role(Role::Slider)
            .aria_label(self.label.clone())
            .aria_value(self.readout.clone())
            .disabled(self.disabled)
            .focusable(!self.disabled);
        if self.disabled {
            self.state.borrow_mut().dragging = false;
            track = track.style(Style::new().opacity(Opacity::Disabled.alpha()));
        }
        if !self.disabled {
            let focus = Style::new().gpui_shadows(vec![aqua_ui::style::GpuiShadowLayer {
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
            }]);
            track = track
                .cursor(aqua_core::CursorIcon::EwResize)
                .focus_visible_style(focus);
            let state = self.state.clone();
            let change = self.on_change.clone();
            let next_layout = layout.clone();
            track = track.on_input(InputKind::PointerDown, EventPhase::Bubble, move |event| {
                let RoutedInput::Native(InputEvent::PointerDown(pointer, PointerButton::Primary)) =
                    &event.input
                else {
                    return InputResponse::default();
                };
                let mut state = state.borrow_mut();
                state.dragging = true;
                let changed = state
                    .seek(pointer.position.x, event.bounds)
                    .then_some(state.value);
                drop(state);
                if let (Some(value), Some(change)) = (changed, &change) {
                    change(value);
                }
                InputResponse {
                    pointer_capture: CaptureRequest::Capture,
                    focus: FocusRequest::Focus,
                    commands: vec![UiCommand::SetStyle {
                        node: event.current_target,
                        style: next_layout.clone(),
                    }],
                    ..InputResponse::handled()
                }
            });
            let state = self.state.clone();
            let change = self.on_change.clone();
            let next_layout = layout.clone();
            track = track.on_input(InputKind::PointerMove, EventPhase::Bubble, move |event| {
                let RoutedInput::Native(InputEvent::PointerMove(pointer)) = &event.input else {
                    return InputResponse::default();
                };
                let mut state = state.borrow_mut();
                if !state.dragging {
                    return InputResponse::default();
                }
                let changed = state
                    .seek(pointer.position.x, event.bounds)
                    .then_some(state.value);
                drop(state);
                if let (Some(value), Some(change)) = (changed, &change) {
                    change(value);
                }
                InputResponse {
                    commands: vec![UiCommand::SetStyle {
                        node: event.current_target,
                        style: next_layout.clone(),
                    }],
                    ..InputResponse::handled()
                }
            });
            let state = self.state.clone();
            track = track.on_input(InputKind::PointerUp, EventPhase::Bubble, move |_| {
                state.borrow_mut().dragging = false;
                InputResponse {
                    pointer_capture: CaptureRequest::Release,
                    ..InputResponse::handled()
                }
            });
            let state = self.state.clone();
            track = track.on_input(InputKind::PointerCancel, EventPhase::Bubble, move |_| {
                state.borrow_mut().dragging = false;
                InputResponse::default()
            });
            let state = self.state.clone();
            let change = self.on_change;
            track = track.on_input(InputKind::KeyDown, EventPhase::Bubble, move |event| {
                let RoutedInput::Native(InputEvent::KeyDown(key)) = &event.input else {
                    return InputResponse::default();
                };
                let command = match key.key {
                    InputKey::Named(NamedKey::Home) => SliderCommand::Start,
                    InputKey::Named(NamedKey::End) => SliderCommand::End,
                    InputKey::Named(NamedKey::Left | NamedKey::Down) => SliderCommand::Previous,
                    InputKey::Named(NamedKey::Right | NamedKey::Up) => SliderCommand::Next,
                    _ => return InputResponse::default(),
                };
                let mut state = state.borrow_mut();
                let value = state.range.command(state.value, command);
                let changed = state.set(value).then_some(state.value);
                drop(state);
                if let (Some(value), Some(change)) = (changed, &change) {
                    change(value);
                }
                InputResponse {
                    stop_propagation: true,
                    commands: vec![UiCommand::SetStyle {
                        node: event.current_target,
                        style: layout.clone(),
                    }],
                    ..InputResponse::handled()
                }
            });
        }
        let text_style = TextStyle {
            families: vec![FontFamily::Named(theme.typography.sans.family.to_owned())],
            size_px: 12.0,
            line_height_px: 16.0,
            weight: FontWeight::MEDIUM,
            ..TextStyle::default()
        };
        let label = Text::new(self.label)
            .style(
                Style::new()
                    .max_width(Length::Px(160.0))
                    .flex_shrink(0.0)
                    .text_style(text_style.clone())
                    .gpui_source_text_color(theme.colors.text_muted.to_reference_text_color())
                    .text_wrap(false),
            )
            .build();
        let readout = Text::new(self.readout)
            .style(
                Style::new()
                    .flex_shrink(0.0)
                    .text_style(TextStyle {
                        weight: FontWeight::NORMAL,
                        ..text_style
                    })
                    .gpui_source_text_color(theme.colors.text.to_reference_text_color())
                    .text_wrap(false),
            )
            .build();
        UiBox::new()
            .gap(12.0)
            .style(Style::new().align(aqua_ui::AlignItems::CENTER))
            .width(Length::Pct(100.0))
            .child(label)
            .child(track)
            .child(readout)
            .build()
    }
}
