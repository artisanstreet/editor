//! A bounded diagram viewport. Parsed SVGs are prepared on the Markdown
//! worker; only the active theme and displayed resolution get a bitmap.
use std::sync::Arc;

use gpui::{
    App, Bounds, ContentMask, Context, CursorStyle, DevicePixels, ElementId, Entity, FocusHandle,
    InteractiveElement as _, IntoElement, MouseButton, MouseMoveEvent, MouseUpEvent,
    ParentElement as _, Pixels, Point, RenderImage, RenderOnce, SharedString, Size,
    StatefulInteractiveElement as _, Styled as _, SvgSize, Window, canvas, div, point, px, size,
};

use super::MermaidDiagram;
use crate::{
    AssetId,
    button::{Button, ButtonVariant},
    theme::{ArtisanTheme, ThemeMode},
};

const MIN_ZOOM: f32 = 0.25;
const MAX_ZOOM: f32 = 8.0;
const ZOOM_STEP: f32 = 1.25;

struct Raster {
    size: Size<DevicePixels>,
    mode: ThemeMode,
    diagram: Arc<MermaidDiagram>,
    image: Arc<RenderImage>,
}

struct ViewState {
    focus: FocusHandle,
    zoom: f32,
    pan: Point<Pixels>,
    drag: Option<Point<Pixels>>,
    viewport: Size<Pixels>,
    displayed: Size<Pixels>,
    raster: Option<Raster>,
}
impl ViewState {
    fn new(cx: &Context<Self>) -> Self {
        cx.on_release(|state, cx| {
            if let Some(raster) = state.raster.take() {
                cx.drop_image(raster.image, None);
            }
        })
        .detach();
        Self {
            focus: cx.focus_handle(),
            zoom: 1.0,
            pan: point(px(0.0), px(0.0)),
            drag: None,
            viewport: Size::default(),
            displayed: Size::default(),
            raster: None,
        }
    }

    fn zoom_by(&mut self, factor: f32) {
        let previous = self.zoom;
        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        self.pan *= self.zoom / previous;
        self.drag = None;
    }
    const fn reset(&mut self) {
        self.zoom = 1.0;
        self.pan = point(px(0.0), px(0.0));
        self.drag = None;
    }
    fn constrain_pan(&mut self) {
        let x = ((self.displayed.width - self.viewport.width) / 2.0).max(px(0.0));
        let y = ((self.displayed.height - self.viewport.height) / 2.0).max(px(0.0));
        self.pan.x = self.pan.x.clamp(-x, x);
        self.pan.y = self.pan.y.clamp(-y, y);
    }
}

#[derive(IntoElement)]
pub(in crate::markdown_renderer) struct DiagramView {
    id: ElementId,
    diagram: Arc<MermaidDiagram>,
    theme: ArtisanTheme,
}
impl DiagramView {
    pub(in crate::markdown_renderer) const fn new(
        id: ElementId,
        diagram: Arc<MermaidDiagram>,
        theme: &ArtisanTheme,
    ) -> Self {
        Self {
            id,
            diagram,
            theme: *theme,
        }
    }
}

impl RenderOnce for DiagramView {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(self.id.clone(), cx, |_, cx| ViewState::new(cx));
        self.content(&state, cx)
    }
}

impl DiagramView {
    fn content(self, state: &Entity<ViewState>, cx: &App) -> impl IntoElement + use<> {
        let focus = state.read(cx).focus.clone();
        let zoom = state.read(cx).zoom;
        let dragging = state.read(cx).drag.is_some();
        let pressing = state.clone();
        let scrolling = state.clone();
        let drawing = state.clone();
        let diagram = Arc::clone(&self.diagram);
        let mode = self.theme.colors.mode;
        let viewport = div()
            .id("viewport")
            .debug_selector(|| "mermaid-viewport".to_owned())
            .aria_label(
                "Mermaid diagram. Drag to pan; Control or Command scroll to zoom; 0 to fit.",
            )
            .track_focus(&focus)
            .relative()
            .w_full()
            .h(px(self.diagram.height.clamp(120.0, 440.0)))
            .overflow_hidden()
            .cursor(if dragging {
                CursorStyle::ClosedHand
            } else {
                CursorStyle::OpenHand
            })
            .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                cx.stop_propagation();
                window.focus(&focus, cx);
                pressing.update(cx, |state, cx| {
                    if event.click_count >= 2 {
                        state.reset();
                    } else {
                        state.drag = Some(event.position);
                    }
                    cx.notify();
                });
            })
            .on_scroll_wheel(move |event, _, cx| {
                if !(event.modifiers.control || event.modifiers.platform) {
                    return;
                }
                cx.stop_propagation();
                let delta = f32::from(event.delta.pixel_delta(px(20.0)).y);
                scrolling.update(cx, |state, cx| {
                    state.zoom_by((-delta * 0.01).exp());
                    cx.notify();
                });
            })
            .on_key_down(on_key(state))
            .child(
                canvas(
                    move |bounds, window, cx| raster(&drawing, &diagram, mode, bounds, window, cx),
                    {
                        let state = state.clone();
                        move |bounds, frame, window, _| {
                            if let Some((image, destination)) = frame {
                                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                                    window
                                        .paint_image(
                                            bounds,
                                            destination,
                                            gpui::Corners::default(),
                                            image,
                                            0,
                                            false,
                                        )
                                        .ok();
                                });
                            }
                            listen_for_drag(&state, window);
                        }
                    },
                )
                .size_full(),
            );
        let toolbar = self.toolbar(state, zoom);
        div()
            .id(self.id)
            .w_full()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(viewport)
            .child(toolbar)
    }
    fn toolbar(&self, state: &Entity<ViewState>, zoom: f32) -> impl IntoElement + use<> {
        let mut toolbar = div().flex().items_center().justify_center().gap(px(4.0));
        for (id, icon, label, factor, disabled) in [
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
            let state = state.clone();
            toolbar = toolbar.child(
                Button::icon(id, self.theme, icon, label)
                    .debug_selector(id)
                    .variant(ButtonVariant::Ghost)
                    .compact(px(28.0))
                    .disabled(disabled)
                    .on_activate(move |_, _, cx| {
                        cx.stop_propagation();
                        state.update(cx, |state, cx| {
                            state.zoom_by(factor);
                            cx.notify();
                        });
                    }),
            );
        }
        let state = state.clone();
        toolbar = toolbar
            .child(
                Button::text("fit", self.theme, "Fit")
                    .debug_selector("mermaid-fit")
                    .variant(ButtonVariant::Ghost)
                    .on_activate(move |_, _, cx| {
                        cx.stop_propagation();
                        state.update(cx, |state, cx| {
                            state.reset();
                            cx.notify();
                        });
                    }),
            )
            .child(
                div()
                    .font_family(self.theme.typography.sans.family)
                    .text_size(px(12.0))
                    .text_color(self.theme.colors.text.to_paint())
                    .child(SharedString::from(format!("{:.0}% of fit", zoom * 100.0))),
            );
        toolbar
    }
}

fn raster(
    state: &Entity<ViewState>,
    diagram: &Arc<MermaidDiagram>,
    mode: ThemeMode,
    bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) -> Option<(Arc<RenderImage>, Bounds<Pixels>)> {
    let renderer = cx.svg_renderer();
    state.update(cx, |state, cx| {
        if state
            .raster
            .as_ref()
            .is_some_and(|raster| !Arc::ptr_eq(&raster.diagram, diagram))
        {
            state.reset();
        }
        state.viewport = bounds.size;
        let fit = (f32::from(bounds.size.width) / diagram.width)
            .min(f32::from(bounds.size.height) / diagram.height)
            .min(1.0);
        state.displayed = size(
            px(diagram.width * fit * state.zoom),
            px(diagram.height * fit * state.zoom),
        );
        state.constrain_pan();
        let destination = Bounds {
            origin: bounds.origin
                + point(
                    (bounds.size.width - state.displayed.width) / 2.0,
                    (bounds.size.height - state.displayed.height) / 2.0,
                )
                + state.pan,
            size: state.displayed,
        };
        // At most 16 MiB per visible diagram; keep the aspect ratio at the cap.
        let width = (f32::from(state.displayed.width) * window.scale_factor())
            .round()
            .max(1.0);
        let height = (f32::from(state.displayed.height) * window.scale_factor())
            .round()
            .max(1.0);
        let cap = (4096.0 / width)
            .min(4096.0 / height)
            .min((4_194_304.0 / (width * height)).sqrt())
            .min(1.0);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "bounded positive raster dimensions"
        )]
        let target = size(
            DevicePixels((width * cap).floor().max(1.0) as i32),
            DevicePixels((height * cap).floor().max(1.0) as i32),
        );
        if !state.raster.as_ref().is_some_and(|raster| {
            raster.size == target && raster.mode == mode && Arc::ptr_eq(&raster.diagram, diagram)
        }) {
            let image = renderer
                .render_parsed(diagram.image(mode), SvgSize::ExactSize(target))
                .ok()?;
            if let Some(old) = state.raster.replace(Raster {
                size: target,
                mode,
                diagram: Arc::clone(diagram),
                image,
            }) {
                cx.drop_image(old.image, Some(window));
            }
        }
        Some((Arc::clone(&state.raster.as_ref()?.image), destination))
    })
}

fn on_key(
    state: &Entity<ViewState>,
) -> impl Fn(&gpui::KeyDownEvent, &mut Window, &mut App) + use<> {
    let state = state.clone();
    move |event, _, cx| {
        if event.keystroke.modifiers.control
            || event.keystroke.modifiers.platform
            || event.keystroke.modifiers.alt
        {
            return;
        }
        state.update(cx, |state, cx| {
            match event.keystroke.key.as_str() {
                "+" | "=" => state.zoom_by(ZOOM_STEP),
                "-" => state.zoom_by(1.0 / ZOOM_STEP),
                "0" => state.reset(),
                "left" => state.pan.x += px(32.0),
                "right" => state.pan.x -= px(32.0),
                "up" => state.pan.y += px(32.0),
                "down" => state.pan.y -= px(32.0),
                _ => return,
            }
            cx.stop_propagation();
            cx.notify();
        });
    }
}

fn listen_for_drag(state: &Entity<ViewState>, window: &mut Window) {
    let moving = state.clone();
    window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
        if phase != gpui::DispatchPhase::Bubble {
            return;
        }
        moving.update(cx, |state, cx| {
            if let Some(last) = state.drag {
                if event.pressed_button == Some(MouseButton::Left) {
                    state.pan += event.position - last;
                    state.drag = Some(event.position);
                } else {
                    state.drag = None;
                }
                cx.notify();
            }
        });
    });
    let releasing = state.clone();
    window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
        if phase != gpui::DispatchPhase::Bubble {
            return;
        }
        releasing.update(cx, |state, cx| {
            if state.drag.take().is_some() {
                cx.notify();
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext as _, Render, TestAppContext};

    struct Example {
        diagram: Arc<MermaidDiagram>,
        state: Entity<ViewState>,
    }
    impl Render for Example {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div().w(px(360.0)).child(
                DiagramView::new(
                    "test-diagram".into(),
                    Arc::clone(&self.diagram),
                    ArtisanTheme::for_mode(ThemeMode::Dark),
                )
                .content(&self.state, cx),
            )
        }
    }

    fn snapshot(
        example: &Entity<Example>,
        cx: &mut gpui::VisualTestContext,
    ) -> (f32, Point<Pixels>, Size<DevicePixels>) {
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            let state = example.read(cx).state.read(cx);
            (
                state.zoom,
                state.pan,
                state.raster.as_ref().expect("raster exists").size,
            )
        })
    }

    #[gpui::test]
    #[expect(
        clippy::too_many_lines,
        reason = "zoom, pan, fit and wheel bounds read as one diagram flow"
    )]
    fn zoom_pan_fit_and_raster_resolution(cx: &mut TestAppContext) {
        let svg = super::super::render("flowchart LR\n A[Start] --> B[Middle] --> C[End]")
            .expect("valid diagram");
        assert!(svg.source.contains("Spline Sans"));
        assert!(!svg.source.contains("Segoe UI"));
        let renderer = gpui::SvgRenderer::new(Arc::new(crate::asset_seam::CatalogAssetSource));
        let diagram = Arc::new(super::super::image(&svg, &renderer).expect("parsed SVG"));
        let (example, cx) = cx.add_window_view(|_, cx| Example {
            diagram,
            state: cx.new(|cx| ViewState::new(cx)),
        });
        let initial = snapshot(&example, cx);
        let zoom_in = cx.debug_bounds("zoom-in").expect("zoom control").center();
        for _ in 0..4 {
            cx.simulate_click(zoom_in, gpui::Modifiers::none());
            cx.run_until_parked();
        }
        let zoomed = snapshot(&example, cx);
        assert!(zoomed.0 > initial.0, "zoom control changes the view");
        assert!(
            zoomed.2.width > initial.2.width,
            "zoom rerasterizes at higher resolution"
        );
        let viewport = cx.debug_bounds("mermaid-viewport").expect("viewport");
        let start = viewport.center();
        let end = start + point(px(-80.0), px(0.0));
        cx.update(|window, cx| {
            window.dispatch_event(
                gpui::PlatformInput::MouseDown(gpui::MouseDownEvent {
                    position: start,
                    button: MouseButton::Left,
                    click_count: 1,
                    modifiers: gpui::Modifiers::none(),
                    ..Default::default()
                }),
                cx,
            );
            window.dispatch_event(
                gpui::PlatformInput::MouseMove(MouseMoveEvent {
                    position: end,
                    pressed_button: Some(MouseButton::Left),
                    modifiers: gpui::Modifiers::none(),
                }),
                cx,
            );
            window.dispatch_event(
                gpui::PlatformInput::MouseUp(MouseUpEvent {
                    position: end,
                    button: MouseButton::Left,
                    modifiers: gpui::Modifiers::none(),
                    click_count: 1,
                }),
                cx,
            );
        });
        let panned = snapshot(&example, cx);
        assert!(panned.1.x < px(0.0), "drag pans the enlarged diagram");
        assert_eq!(panned.2, zoomed.2, "panning keeps the raster resolution");
        assert_eq!(
            cx.debug_bounds("mermaid-viewport"),
            Some(viewport),
            "zoom keeps the transcript layout stable"
        );
        let fit = cx
            .debug_bounds("mermaid-fit")
            .expect("fit control")
            .center();
        cx.simulate_click(fit, gpui::Modifiers::none());
        let fitted = snapshot(&example, cx);
        assert!((fitted.0 - 1.0).abs() < f32::EPSILON);
        assert_eq!(fitted.1, point(px(0.0), px(0.0)));
        assert_eq!(fitted.2, initial.2);

        // Plain scrolling remains available to the transcript; modified
        // scrolling zooms, with hard bounds for extreme wheel deltas.
        for (control, delta, expected) in [
            (false, -20.0, 1.0),
            (true, -2000.0, MAX_ZOOM),
            (true, 2000.0, MIN_ZOOM),
        ] {
            cx.update(|window, cx| {
                window.dispatch_event(
                    gpui::PlatformInput::ScrollWheel(gpui::ScrollWheelEvent {
                        position: viewport.center(),
                        delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(delta))),
                        modifiers: gpui::Modifiers {
                            control,
                            ..Default::default()
                        },
                        ..Default::default()
                    }),
                    cx,
                );
            });
            let current = snapshot(&example, cx);
            assert!((current.0 - expected).abs() < f32::EPSILON);
            assert!(current.2.width.0 <= 4096 && current.2.height.0 <= 4096);
            assert!(i64::from(current.2.width.0) * i64::from(current.2.height.0) <= 4_194_304);
        }
        cx.simulate_click(viewport.center(), gpui::Modifiers::none());
        cx.update(|window, cx| {
            window.dispatch_event(
                gpui::PlatformInput::KeyDown(gpui::KeyDownEvent {
                    keystroke: gpui::Keystroke::parse("0").expect("reset key"),
                    is_held: false,
                    prefer_character_input: false,
                }),
                cx,
            );
        });
        assert!(
            (snapshot(&example, cx).0 - 1.0).abs() < f32::EPSILON,
            "keyboard reset fits the diagram"
        );
    }
}
