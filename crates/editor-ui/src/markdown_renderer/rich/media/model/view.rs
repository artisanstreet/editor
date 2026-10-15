//! The model viewer: a transparent canvas the reader turns.
//!
//! The frame is a hairline-edged rectangle with nothing behind the model,
//! so it sits on the transcript like an image. Dragging orbits the model,
//! Shift-drag or the middle button pans, Ctrl-scroll (⌘ on macOS) zooms
//! (a plain scroll keeps scrolling the transcript), and double-clicking
//! resets the view. Once pressed, the frame takes the keyboard: arrows
//! orbit (Shift for finer steps), `+`/`-` zoom, `0` resets. Below the frame
//! a mode row switches between shaded and wireframe, and a muted line names
//! the format and the triangle count.
//!
//! Where the window's renderer draws meshes (wgpu: Linux and Windows), the
//! model is painted with `Window::paint_mesh` every frame: the geometry
//! stays on the GPU and only the camera changes, so it turns at the
//! display's frame rate. Elsewhere (macOS's Metal renderer) the software
//! rasterizer draws it on the background executor: the newest view wins,
//! so while one render runs, changes wait and its completion asks for the
//! latest view; dragging renders at one sample per pixel, at rest 2×2.

use std::sync::Arc;

use gpui::{
    AnyElement, App, Context, CursorStyle, Div, Entity, FocusHandle, ImageSource, IntoElement,
    KeyDownEvent, MouseButton, MouseMoveEvent, MouseUpEvent, ObjectFit, ParentElement as _, Pixels,
    Point, RenderImage, RenderOnce, SharedString, Size, Styled as _, StyledImage as _, Task,
    Window, canvas, div, img, prelude::*, px,
};
use serde::Deserialize;

use super::super::super::actions::{CopyEntry, show_copy_menu};
use super::super::super::kit;
use super::super::picture::{MAX_FRAME_HEIGHT, SizeHint, placeholder};
use super::super::source::MediaSource;
use super::formats::{Format, Model};
use super::load::{self, Loaded};
use super::mesh::{Up, Vec3};
use super::raster::{self, Camera, Look};
use crate::button::{Button, ButtonVariant};
use crate::segmented_control::{Segment, SegmentedControl};
use crate::theme::{ArtisanTheme, ThemeMode};
use editor_assets::AssetId;

/// The frame's aspect ratio unless the spec hints one.
const DEFAULT_ASPECT: f32 = 16.0 / 10.0;

/// Degrees of orbit per pixel dragged.
const ORBIT_PER_PIXEL: f32 = 0.45;

/// Degrees per arrow press; Shift for `FINE_STEP`.
const STEP: f32 = 10.0;
const FINE_STEP: f32 = 2.0;

/// Zoom per pixel of Ctrl-scroll, and per `+`/`-` press.
const ZOOM_PER_PIXEL: f32 = 0.0025;
const ZOOM_STEP: f32 = 0.85;

/// The farthest the look-at point pans from the model's center, in radii.
const MAX_PAN: f32 = 1.5;

/// The most samples one frame renders; larger frames drop to one sample
/// per pixel.
const MAX_SAMPLES: u32 = 3_000_000;

/// The largest side of a rendered frame, in device pixels.
const MAX_SIDE: f32 = 2400.0;

/// How the surface draws.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(super) enum Shading {
    #[default]
    Shaded,
    Wireframe,
}

impl Shading {
    const ALL: [Self; 2] = [Self::Shaded, Self::Wireframe];

    fn label(self) -> &'static str {
        match self {
            Self::Shaded => "Shaded",
            Self::Wireframe => "Wireframe",
        }
    }
}

#[derive(IntoElement)]
pub(super) struct ModelView {
    pub(super) media: Option<super::super::source::MediaFetcher>,
    pub(super) id: SharedString,
    pub(super) theme: ArtisanTheme,
    pub(super) src: MediaSource,
    pub(super) name: SharedString,
    pub(super) format: Option<Format>,
    pub(super) up: Option<Up>,
    pub(super) color: Option<Vec3>,
    pub(super) shading: Shading,
    pub(super) yaw: Option<f32>,
    pub(super) pitch: Option<f32>,
    pub(super) hint: SizeHint,
}

/// Everything one rendered frame depends on.
#[derive(Clone, Copy, Debug, PartialEq)]
struct FrameKey {
    /// The model's identity: its parsed mesh's address.
    model: usize,
    camera: Camera,
    look: Look,
    width: u32,
    height: u32,
    samples: u32,
}

struct Drag {
    last: Point<Pixels>,
    pan: bool,
}

struct ViewerState {
    camera: Camera,
    /// The spec's view, which reset returns to.
    initial: Camera,
    shading: Shading,
    focus: FocusHandle,
    drag: Option<Drag>,
    /// The frame's size as last laid out, to turn pixels into pan.
    size: Size<Pixels>,
    /// The newest finished frame.
    shown: Option<(Arc<RenderImage>, FrameKey)>,
    /// The frame being rendered.
    rendering: Option<FrameKey>,
    /// The render in flight; dropping it cancels the render.
    render_task: Option<Task<()>>,
}

impl ViewerState {
    fn new(camera: Camera, shading: Shading, cx: &mut Context<Self>) -> Self {
        // The frame's pixels live in each window's sprite atlas until dropped.
        cx.on_release(|state, cx| {
            if let Some((image, _)) = state.shown.take() {
                cx.drop_image(image, None);
            }
        })
        .detach();
        Self {
            camera,
            initial: camera,
            shading,
            focus: cx.focus_handle(),
            drag: None,
            size: Size::default(),
            shown: None,
            rendering: None,
            render_task: None,
        }
    }

    /// Starts rendering `key` unless it is on screen or a render is running.
    fn request(&mut self, key: FrameKey, model: &Arc<Model>, cx: &mut Context<Self>) {
        let current = self.shown.as_ref().is_some_and(|(_, shown)| *shown == key);
        if current || self.rendering.is_some() {
            return;
        }
        self.rendering = Some(key);
        let model = Arc::clone(model);
        self.render_task = Some(cx.spawn(async move |this, cx| {
            let image = cx
                .background_executor()
                .spawn(async move {
                    let frame = raster::render(
                        &model.mesh,
                        &key.camera,
                        &key.look,
                        key.width,
                        key.height,
                        key.samples,
                    );
                    image::RgbaImage::from_raw(frame.width, frame.height, frame.pixels)
                        .map(|buffer| Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
                })
                .await;
            let _ = this.update(cx, |state, cx| {
                state.rendering = None;
                if let Some(image) = image
                    && let Some((old, _)) = state.shown.replace((image, key))
                {
                    cx.drop_image(old, None);
                }
                cx.notify();
            });
        }));
    }

    /// Follows the pointer while dragging; whether the view moved.
    fn drag_to(&mut self, position: Point<Pixels>) -> bool {
        let Some(drag) = &mut self.drag else {
            return false;
        };
        let dx = f32::from(position.x - drag.last.x);
        let dy = f32::from(position.y - drag.last.y);
        drag.last = position;
        if drag.pan {
            let focal = Camera::focal(
                f32::from(self.size.width).max(1.0),
                f32::from(self.size.height).max(1.0),
            );
            let per_pixel = self.camera.distance / focal;
            let (x, y) = self.camera.pan;
            self.camera.pan = (
                (x - dx * per_pixel).clamp(-MAX_PAN, MAX_PAN),
                (y + dy * per_pixel).clamp(-MAX_PAN, MAX_PAN),
            );
        } else {
            self.orbit(dx * ORBIT_PER_PIXEL, dy * ORBIT_PER_PIXEL);
        }
        true
    }

    fn orbit(&mut self, yaw: f32, pitch: f32) {
        self.camera.yaw = (self.camera.yaw + yaw).rem_euclid(360.0);
        self.camera.pitch = (self.camera.pitch + pitch).clamp(-89.0, 89.0);
    }

    fn zoom(&mut self, factor: f32) {
        self.camera.distance =
            (self.camera.distance * factor).clamp(Camera::NEAREST, Camera::FARTHEST);
    }

    fn reset(&mut self) {
        self.camera = self.initial;
        self.drag = None;
    }

    /// Applies a key press; whether anything changed.
    fn key(&mut self, event: &KeyDownEvent) -> bool {
        let step = if event.keystroke.modifiers.shift {
            FINE_STEP
        } else {
            STEP
        };
        match event.keystroke.key.as_str() {
            "left" => self.orbit(-step, 0.0),
            "right" => self.orbit(step, 0.0),
            "up" => self.orbit(0.0, -step),
            "down" => self.orbit(0.0, step),
            "=" | "+" => self.zoom(ZOOM_STEP),
            "-" => self.zoom(1.0 / ZOOM_STEP),
            "0" => self.reset(),
            _ => return false,
        }
        true
    }
}

impl ModelView {
    fn part(&self, part: impl std::fmt::Display) -> gpui::ElementId {
        kit::part(&self.id, part)
    }

    /// The surface colors for this theme.
    fn look(&self, shading: Shading) -> Look {
        let clay = match self.theme.mode {
            ThemeMode::Dark => Vec3::new(0.52, 0.5, 0.48),
            ThemeMode::Light => Vec3::new(0.4, 0.385, 0.37),
        };
        Look {
            base: self.color.unwrap_or(clay),
            edges: (shading == Shading::Wireframe).then_some((Vec3::new(0.01, 0.01, 0.012), 0.6)),
        }
    }

    /// The frame's aspect ratio and the widest it grows.
    fn frame(&self) -> (f32, Pixels) {
        let aspect = match (self.hint.width, self.hint.height) {
            (Some(width), Some(height)) if width > 0.0 && height > 0.0 => {
                (width / height).clamp(0.5, 3.0)
            }
            _ => DEFAULT_ASPECT,
        };
        (aspect, px(MAX_FRAME_HEIGHT * aspect))
    }
}

impl RenderOnce for ModelView {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = self.theme;
        let (aspect, max_width) = self.frame();
        let model = match load::load(
            window,
            cx,
            &self.id,
            &self.src,
            self.format,
            self.up,
            self.media.as_ref(),
        ) {
            Loaded::Ready(model) => model,
            Loaded::Failed(reason) => {
                return div()
                    .w_full()
                    .max_w(max_width)
                    .child(crate::media_frame::MediaFrame::new(
                        theme,
                        aspect,
                        placeholder(&theme, Some(AssetId::TABLER_FILE_OFF), reason),
                    ))
                    .into_any_element();
            }
            Loaded::Loading => {
                return div()
                    .w_full()
                    .max_w(max_width)
                    .child(crate::media_frame::MediaFrame::new(
                        theme,
                        aspect,
                        placeholder(&theme, None, "Loading model…"),
                    ))
                    .into_any_element();
            }
        };
        let initial = Camera::new(
            self.yaw.unwrap_or(Camera::DEFAULT_YAW),
            self.pitch.unwrap_or(Camera::DEFAULT_PITCH),
        );
        let shading = self.shading;
        let state = window.use_keyed_state(self.part("state"), cx, |_, cx| {
            ViewerState::new(initial, shading, cx)
        });
        let (shading, moved, shown) = {
            let state = state.read(cx);
            (
                state.shading,
                state.camera != state.initial,
                state.shown.as_ref().map(|(image, _)| Arc::clone(image)),
            )
        };
        let look = self.look(shading);
        let drawing = if window.supports_meshes() {
            canvas(record_size(&state), paint_on_gpu(&state, &model, look))
                .absolute()
                .size_full()
                .into_any_element()
        } else {
            software_layers(&state, &model, look, shown)
        };
        div()
            .w_full()
            .max_w(max_width)
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(self.canvas(&state, drawing, aspect, cx))
            .child(self.toolbar(&state, &model, shading, moved))
            .into_any_element()
    }
}

impl ModelView {
    /// The frame around `drawing`, with every pointer and key binding.
    fn canvas(
        &self,
        state: &Entity<ViewerState>,
        drawing: AnyElement,
        aspect: f32,
        cx: &App,
    ) -> AnyElement {
        let theme = self.theme;
        let (focus, dragging) = {
            let state = state.read(cx);
            (state.focus.clone(), state.drag.is_some())
        };
        let group = SharedString::from(format!("{}-frame", self.id));
        let path = self.src.clone();
        let copy = move |event: &gpui::MouseDownEvent, window: &mut Window, cx: &mut App| {
            cx.stop_propagation();
            let entries = vec![CopyEntry::primary("Copy Model Path", path.clone())];
            show_copy_menu(event.position, entries, window, cx);
        };
        let content = div()
            .relative()
            .size_full()
            .id(self.part("frame"))
            .aria_label(self.name.clone())
            .group(group.clone())
            .track_focus(&focus)
            .cursor(if dragging {
                CursorStyle::ClosedHand
            } else {
                CursorStyle::OpenHand
            })
            .on_mouse_down(MouseButton::Left, on_press(state, &focus, false))
            .on_mouse_down(MouseButton::Middle, on_press(state, &focus, true))
            .on_mouse_down(MouseButton::Right, copy)
            .on_scroll_wheel(on_wheel(state))
            .on_key_down(on_key(state))
            .child(drawing)
            .child(hint(&theme, &group, dragging));
        crate::media_frame::MediaFrame::new(theme, aspect, content).into_any_element()
    }

    /// The mode row, a reset while the view has moved, and the facts.
    fn toolbar(
        &self,
        state: &Entity<ViewerState>,
        model: &Model,
        chosen: Shading,
        moved: bool,
    ) -> Div {
        let theme = self.theme;
        let shading_state = state.clone();
        let entries = SegmentedControl::new(
            self.part("shading"),
            theme,
            "Model shading",
            chosen,
            Segment::new(Shading::ALL[0], Shading::ALL[0].label()),
        )
        .options(
            Shading::ALL[1..]
                .iter()
                .map(|shading| Segment::new(*shading, shading.label())),
        )
        .on_change(move |shading, _, cx| {
            shading_state.update(cx, |state, cx| {
                if state.shading != shading {
                    state.shading = shading;
                    cx.notify();
                }
            });
        });
        let reset = moved.then(|| {
            let state = state.clone();
            Button::text(self.part("reset"), theme, "Reset view")
                .variant(ButtonVariant::Ghost)
                .on_activate(move |_, _, cx| {
                    state.update(cx, |state, cx| {
                        state.reset();
                        cx.notify();
                    });
                })
        });
        let triangles = model.triangles;
        #[expect(clippy::cast_precision_loss, reason = "a displayed count")]
        let count = kit::format_number(triangles as f64);
        let noun = if triangles == 1 {
            "triangle"
        } else {
            "triangles"
        };
        div()
            .w_full()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(8.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.0))
                    .child(entries)
                    .children(reset),
            )
            .child(
                kit::muted(&theme, format!("{} · {count} {noun}", model.format.label()))
                    .text_size(px(12.0))
                    .whitespace_nowrap(),
            )
    }
}

/// Pressing starts an orbit, or a pan with Shift or the middle button;
/// double-clicking resets the view.
fn on_press(
    state: &Entity<ViewerState>,
    focus: &FocusHandle,
    pan: bool,
) -> impl Fn(&gpui::MouseDownEvent, &mut Window, &mut App) + 'static {
    let state = state.clone();
    let focus = focus.clone();
    move |event, window, cx| {
        cx.stop_propagation();
        window.focus(&focus, cx);
        state.update(cx, |state, cx| {
            if !pan && event.click_count >= 2 {
                state.reset();
            } else {
                state.drag = Some(Drag {
                    last: event.position,
                    pan: pan || event.modifiers.shift,
                });
            }
            cx.notify();
        });
    }
}

fn on_key(state: &Entity<ViewerState>) -> impl Fn(&KeyDownEvent, &mut Window, &mut App) + 'static {
    let state = state.clone();
    move |event, _, cx| {
        state.update(cx, |state, cx| {
            if state.key(event) {
                cx.stop_propagation();
                cx.notify();
            }
        });
    }
}

/// Ctrl-scroll (⌘ on macOS) zooms; a plain scroll belongs to the transcript.
fn on_wheel(
    state: &Entity<ViewerState>,
) -> impl Fn(&gpui::ScrollWheelEvent, &mut Window, &mut App) + 'static {
    let state = state.clone();
    move |event, _, cx| {
        if !(event.modifiers.control || event.modifiers.platform) {
            return;
        }
        cx.stop_propagation();
        let delta = f32::from(event.delta.pixel_delta(px(20.0)).y);
        state.update(cx, |state, cx| {
            state.zoom((-delta * ZOOM_PER_PIXEL).exp());
            cx.notify();
        });
    }
}

/// The software path's layers: a canvas that asks for frames at its size,
/// under the newest finished frame.
fn software_layers(
    state: &Entity<ViewerState>,
    model: &Arc<Model>,
    look: Look,
    shown: Option<Arc<RenderImage>>,
) -> AnyElement {
    div()
        .absolute()
        .size_full()
        .child(
            canvas(request_frame(state, model, look), follow_pointer(state))
                .absolute()
                .size_full(),
        )
        .children(shown.map(|image| {
            img(ImageSource::Render(image))
                .absolute()
                .size_full()
                .object_fit(ObjectFit::Contain)
        }))
        .into_any_element()
}

/// Before paint, asks for a frame at the canvas's device-pixel size.
fn request_frame(
    state: &Entity<ViewerState>,
    model: &Arc<Model>,
    look: Look,
) -> impl FnOnce(gpui::Bounds<Pixels>, &mut Window, &mut App) + 'static {
    let state = state.clone();
    let model = Arc::clone(model);
    move |bounds, window, cx| {
        let scale = window.scale_factor();
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "clamped to 1..=MAX_SIDE first"
        )]
        let side = |length: Pixels| (f32::from(length) * scale).round().clamp(1.0, MAX_SIDE) as u32;
        let (width, height) = (side(bounds.size.width), side(bounds.size.height));
        state.update(cx, |state, cx| {
            state.size = bounds.size;
            let samples = if state.drag.is_some() || width * height * 4 > MAX_SAMPLES {
                1
            } else {
                2
            };
            let key = FrameKey {
                model: Arc::as_ptr(&model).addr(),
                camera: state.camera,
                look,
                width,
                height,
                samples,
            };
            state.request(key, &model, cx);
        });
    }
}

/// While painting, listens on the window so a drag follows the pointer
/// outside the frame.
fn follow_pointer(
    state: &Entity<ViewerState>,
) -> impl FnOnce(gpui::Bounds<Pixels>, (), &mut Window, &mut App) + 'static {
    let state = state.clone();
    move |_, (), window, _| listen_for_drag(&state, window)
}

/// Before paint, records the frame's size, which pans are measured in.
fn record_size(
    state: &Entity<ViewerState>,
) -> impl FnOnce(gpui::Bounds<Pixels>, &mut Window, &mut App) + 'static {
    let state = state.clone();
    move |bounds, _, cx| state.update(cx, |state, _| state.size = bounds.size)
}

/// Paints the mesh through the GPU renderer at the current view: no
/// rendering ahead of time, so every drag moves it on the next frame.
fn paint_on_gpu(
    state: &Entity<ViewerState>,
    model: &Arc<Model>,
    look: Look,
) -> impl FnOnce(gpui::Bounds<Pixels>, (), &mut Window, &mut App) + 'static {
    let state = state.clone();
    let mesh = model.gpu.clone();
    move |bounds, (), window, cx| {
        let Some(mesh) = mesh else {
            return;
        };
        let camera = state.read(cx).camera;
        let base = look.base;
        let style = gpui::MeshStyle {
            view: camera.view_matrix(),
            projection: Camera::projection_matrix(
                f32::from(bounds.size.width).max(1.0),
                f32::from(bounds.size.height).max(1.0),
            ),
            base_color: [base.x, base.y, base.z],
            edge_color: look.edges.map_or([0.0; 4], |(edge, opacity)| {
                [edge.x, edge.y, edge.z, opacity]
            }),
        };
        // The frame's border sits outside the canvas: round to the inner edge.
        let radius = (kit::inner_radius() - px(1.0)).max(px(0.0));
        window.paint_mesh(bounds, gpui::Corners::all(radius), mesh, style);
        listen_for_drag(&state, window);
    }
}

/// Listens on the window so a drag follows the pointer outside the frame.
fn listen_for_drag(state: &Entity<ViewerState>, window: &mut Window) {
    let moving = state.clone();
    window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
        if phase != gpui::DispatchPhase::Bubble {
            return;
        }
        moving.update(cx, |state, cx| {
            if state.drag_to(event.position) {
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

/// How to turn the model, shown while the pointer is over the frame.
fn hint(theme: &ArtisanTheme, group: &SharedString, dragging: bool) -> Div {
    let shortcut = if cfg!(target_os = "macos") {
        "⌘-scroll"
    } else {
        "Ctrl-scroll"
    };
    div()
        .absolute()
        .bottom(px(8.0))
        .left(px(10.0))
        .opacity(0.0)
        .when(!dragging, |hint| {
            hint.group_hover(group.clone(), |style| style.opacity(1.0))
        })
        .child(
            kit::muted(
                theme,
                format!("Drag to turn · Shift-drag to pan · {shortcut} to zoom"),
            )
            .text_size(px(12.0)),
        )
}
