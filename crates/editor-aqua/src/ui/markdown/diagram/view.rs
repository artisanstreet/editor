use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use aqua_core::{
    CursorIcon, DisplayItem, InputEvent, Key, Modifiers, NamedKey, Point, PointerButton, Rect,
};
use aqua_text::FontWeight;
use aqua_ui::element::{Element, ui};
use aqua_ui::input::{
    CaptureRequest, EventPhase, FocusRequest, InputKind, InputResponse, RoutedInput,
};
use aqua_ui::style::{Length, Style};
use aqua_ui::{AlignItems, JustifyContent};
use editor_assets::AssetId;

use crate::ui::button::{Button, ButtonVariant};
use crate::ui::markdown::DiagramSvg;
use crate::ui::rich::narrative::callout::text;
use crate::ui::theme::{ArtisanTheme, ThemeMode};

const MIN_ZOOM: f32 = 0.25;
const MAX_ZOOM: f32 = 8.0;
const ZOOM_STEP: f32 = 1.25;
const GPUI_LINE_PX: f32 = 20.0;
const AQUA_LINE_PX: f32 = 16.0;
const WHEEL_ZOOM_PER_PIXEL: f32 = 0.01 * GPUI_LINE_PX / AQUA_LINE_PX;
const KEY_PAN_PX: f32 = 32.0;
const MIN_VIEWPORT_PX: f32 = 120.0;
const MAX_VIEWPORT_PX: f32 = 440.0;

#[derive(Default)]
pub(crate) struct DiagramState {
    diagram: Option<Arc<DiagramSvg>>,
    zoom: f32,
    pan: Point,
    drag: Option<Point>,
}

impl DiagramState {
    pub(crate) fn new() -> Self {
        Self {
            zoom: 1.0,
            ..Self::default()
        }
    }

    fn sync(&mut self, diagram: &Arc<DiagramSvg>) {
        if !self
            .diagram
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, diagram))
        {
            self.reset();
            self.diagram = Some(Arc::clone(diagram));
        }
    }

    fn zoom_by(&mut self, factor: f32) {
        let previous = self.zoom;
        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let ratio = self.zoom / previous;
        self.pan = Point {
            x: self.pan.x * ratio,
            y: self.pan.y * ratio,
        };
        self.drag = None;
    }

    fn reset(&mut self) {
        self.zoom = 1.0;
        self.pan = Point { x: 0.0, y: 0.0 };
        self.drag = None;
    }

    fn destination(&mut self, bounds: Rect, diagram: &DiagramSvg) -> Rect {
        let fit = (bounds.w / diagram.width())
            .min(bounds.h / diagram.height())
            .min(1.0);
        let displayed = (
            diagram.width() * fit * self.zoom,
            diagram.height() * fit * self.zoom,
        );
        Rect {
            x: bounds.x + (bounds.w - displayed.0) / 2.0 + self.pan.x,
            y: bounds.y + (bounds.h - displayed.1) / 2.0 + self.pan.y,
            w: displayed.0,
            h: displayed.1,
        }
    }

    fn key(&mut self, key: Key) -> bool {
        match key {
            Key::Char('+' | '=') => self.zoom_by(ZOOM_STEP),
            Key::Char('-') => self.zoom_by(1.0 / ZOOM_STEP),
            Key::Char('0') => self.reset(),
            Key::Named(NamedKey::Left) => self.pan.x += KEY_PAN_PX,
            Key::Named(NamedKey::Right) => self.pan.x -= KEY_PAN_PX,
            Key::Named(NamedKey::Up) => self.pan.y += KEY_PAN_PX,
            Key::Named(NamedKey::Down) => self.pan.y -= KEY_PAN_PX,
            _ => return false,
        }
        true
    }

    fn drag_to(&mut self, point: Point) -> bool {
        let Some(last) = self.drag else {
            return false;
        };
        self.pan = Point {
            x: self.pan.x + point.x - last.x,
            y: self.pan.y + point.y - last.y,
        };
        self.drag = Some(point);
        true
    }
}

type Shared = Rc<RefCell<DiagramState>>;

fn changing(
    state: &Shared,
    invalidate: &Rc<dyn Fn()>,
    change: impl Fn(&mut DiagramState) -> bool + 'static,
) -> impl Fn() + 'static {
    let state = Rc::clone(state);
    let invalidate = Rc::clone(invalidate);
    move || {
        if change(&mut state.borrow_mut()) {
            invalidate();
        }
    }
}

pub(crate) fn view(
    theme: &ArtisanTheme,
    id: &str,
    diagram: &Arc<DiagramSvg>,
    state: &Shared,
    invalidate: &Rc<dyn Fn()>,
) -> Element {
    state.borrow_mut().sync(diagram);
    let (zoom, dragging) = {
        let state = state.borrow();
        (state.zoom, state.drag.is_some())
    };
    let source = match theme.colors.mode {
        ThemeMode::Dark => Arc::clone(&diagram.source),
        ThemeMode::Light => Arc::clone(&diagram.light_source),
    };
    let paint_state = Rc::clone(state);
    let paint_diagram = Arc::clone(diagram);
    let canvas = ui::Canvas::new()
        .width(Length::Pct(100.0))
        .height(Length::Pct(100.0))
        .on_paint(move |bounds| {
            let destination = paint_state.borrow_mut().destination(bounds, &paint_diagram);
            vec![DisplayItem::GpuiSvg {
                bounds: destination,
                source: Arc::clone(&source),
                tint: None,
                opacity: 1.0,
            }]
        })
        .build();
    let down_state = Rc::clone(state);
    let down_invalidate = Rc::clone(invalidate);
    let move_state = Rc::clone(state);
    let move_invalidate = Rc::clone(invalidate);
    let release = changing(state, invalidate, |state| state.drag.take().is_some());
    let cancel = changing(state, invalidate, |state| state.drag.take().is_some());
    let wheel_state = Rc::clone(state);
    let wheel_invalidate = Rc::clone(invalidate);
    let key_state = Rc::clone(state);
    let key_invalidate = Rc::clone(invalidate);
    let viewport = ui::Box::new()
        .width(Length::Pct(100.0))
        .height(diagram.height().clamp(MIN_VIEWPORT_PX, MAX_VIEWPORT_PX))
        .clip(true)
        .child(canvas)
        .build()
        .with_key(format!("{id}-viewport"))
        .focusable(true)
        .aria_label("Mermaid diagram. Drag to pan; scroll to zoom; 0 to fit.")
        .cursor(if dragging {
            CursorIcon::Grabbing
        } else {
            CursorIcon::Grab
        })
        .on_input(InputKind::PointerDown, EventPhase::Bubble, move |event| {
            let RoutedInput::Native(InputEvent::PointerDown(pointer, PointerButton::Primary)) =
                &event.input
            else {
                return InputResponse::default();
            };
            {
                let mut state = down_state.borrow_mut();
                if event.click_count.unwrap_or(1) >= 2 {
                    state.reset();
                } else {
                    state.drag = Some(pointer.position);
                }
            }
            down_invalidate();
            InputResponse {
                stop_propagation: true,
                focus: FocusRequest::Focus,
                pointer_capture: CaptureRequest::Capture,
                ..InputResponse::handled()
            }
        })
        .on_input(InputKind::PointerMove, EventPhase::Bubble, move |event| {
            let RoutedInput::Native(InputEvent::PointerMove(pointer)) = &event.input else {
                return InputResponse::default();
            };
            if move_state.borrow_mut().drag_to(pointer.position) {
                move_invalidate();
                return InputResponse {
                    stop_propagation: true,
                    ..InputResponse::handled()
                };
            }
            InputResponse::default()
        })
        .on_input(InputKind::PointerUp, EventPhase::Bubble, move |_| {
            release();
            InputResponse {
                pointer_capture: CaptureRequest::Release,
                stop_propagation: true,
                ..InputResponse::handled()
            }
        })
        .on_input(InputKind::PointerCancel, EventPhase::Bubble, move |_| {
            cancel();
            InputResponse {
                pointer_capture: CaptureRequest::Release,
                stop_propagation: true,
                ..InputResponse::handled()
            }
        })
        .on_input(InputKind::Scroll, EventPhase::Bubble, move |event| {
            let RoutedInput::Native(InputEvent::Scroll { delta, .. }) = &event.input else {
                return InputResponse::default();
            };
            wheel_state
                .borrow_mut()
                .zoom_by((-delta.y * WHEEL_ZOOM_PER_PIXEL).exp());
            wheel_invalidate();
            InputResponse {
                stop_propagation: true,
                ..InputResponse::handled()
            }
        })
        .on_input(InputKind::KeyDown, EventPhase::Bubble, move |event| {
            let RoutedInput::Native(InputEvent::KeyDown(key)) = &event.input else {
                return InputResponse::default();
            };
            if [Modifiers::CONTROL, Modifiers::LOGO, Modifiers::ALT]
                .into_iter()
                .any(|modifier| key.modifiers.contains(modifier))
            {
                return InputResponse::default();
            }
            if !key_state.borrow_mut().key(key.key) {
                return InputResponse::default();
            }
            key_invalidate();
            InputResponse {
                stop_propagation: true,
                ..InputResponse::handled()
            }
        });
    ui::Box::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .gap(8.0)
        .child(viewport)
        .child(toolbar(theme, id, zoom, state, invalidate))
        .build()
}

fn toolbar(
    theme: &ArtisanTheme,
    id: &str,
    zoom: f32,
    state: &Shared,
    invalidate: &Rc<dyn Fn()>,
) -> Element {
    let mut row = ui::Box::new()
        .layout("row")
        .style(
            Style::new()
                .align(AlignItems::CENTER)
                .justify(JustifyContent::CENTER),
        )
        .width(Length::Pct(100.0))
        .gap(4.0);
    for (name, icon, label, factor, disabled) in [
        (
            "zoom-out",
            AssetId::TABLER_ZOOM_OUT,
            "Zoom out",
            1.0 / ZOOM_STEP,
            zoom <= MIN_ZOOM,
        ),
        (
            "zoom-in",
            AssetId::TABLER_ZOOM_IN,
            "Zoom in",
            ZOOM_STEP,
            zoom >= MAX_ZOOM,
        ),
    ] {
        let apply = changing(state, invalidate, move |state| {
            state.zoom_by(factor);
            true
        });
        row = row.child(
            Button::icon(format!("{id}-{name}"), *theme, icon, label)
                .debug_selector(name)
                .variant(ButtonVariant::Ghost)
                .compact(28.0)
                .disabled(disabled)
                .on_activate(move |_| apply())
                .build(),
        );
    }
    let fit = changing(state, invalidate, |state| {
        state.reset();
        true
    });
    row.child(
        Button::text(format!("{id}-fit"), *theme, "Fit")
            .debug_selector("mermaid-fit")
            .variant(ButtonVariant::Ghost)
            .on_activate(move |_| fit())
            .build(),
    )
    .child(text(
        theme,
        &format!("{:.0}% of fit", zoom * 100.0),
        12.0,
        16.0,
        FontWeight::NORMAL,
        theme.colors.text,
    ))
    .build()
}

#[cfg(test)]
#[path = "../../../../tests/markdown/diagram/view.rs"]
mod tests;
