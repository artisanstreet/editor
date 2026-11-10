use super::{ModelBlock, Shading};
use editor_core::{delivery::Delivery, runtime};
use crate::ui::media::{
    model::{
        Camera, Look,
        load::{Geometry, LoadedModel, ModelAssets, RendererCapability},
        render,
    },
    source::MediaFetcher,
};
use aqua::images::{ImageAssets, ImageHandle};
use aqua_core::gpui_mesh::GpuiMeshStyle;
use aqua_core::{
    CornerRadii, DisplayItem, InputEvent, Key, Modifiers, NamedKey, PointerButton, Rect,
};
use aqua_ui::{
    element::{Element, Media, ui},
    input::{CaptureRequest, EventPhase, FocusRequest, InputKind, InputResponse, RoutedInput},
    style::{Length, Style},
};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    num::NonZeroUsize,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::task::AbortHandle;

static NEXT_WAITER: AtomicU64 = AtomicU64::new(1);

pub(crate) type FrameFactory = Rc<dyn Fn(Element, f32, f32, &str, Option<bool>) -> Element>;
pub(crate) type PlaceholderFactory = Rc<dyn Fn(&str, bool) -> Element>;
pub(crate) type ToolbarFactory =
    Rc<dyn Fn(Shading, bool, &str, Rc<dyn Fn(Shading)>, Rc<dyn Fn()>) -> Element>;
pub(crate) type CopyFactory = Rc<dyn Fn(aqua_core::Point, &str)>;

pub(crate) struct ModelServices {
    pub assets: ModelAssets,
    pub capability: Rc<dyn Fn() -> Option<RendererCapability>>,
    pub frame: FrameFactory,
    pub placeholder: PlaceholderFactory,
    pub toolbar: ToolbarFactory,
    pub copy: CopyFactory,
    pub invalidate: Rc<dyn Fn()>,
}

#[derive(Clone, Copy, PartialEq)]
struct FrameKey {
    model: usize,
    camera: Camera,
    look: Look,
    width: u32,
    height: u32,
    samples: u32,
}
struct Drag {
    last: aqua_core::Point,
    pan: bool,
}
struct Entry {
    owner: Option<(String, Arc<[u64]>)>,
    model: Option<usize>,
    initial: Camera,
    camera: Camera,
    shading: Shading,
    drag: Option<Drag>,
    bounds: Rc<Cell<Option<Rect>>>,
    shown: Option<(ImageHandle, FrameKey)>,
    rendering: Option<(FrameKey, AbortHandle)>,
    generation: u64,
}
struct Completion {
    id: String,
    generation: u64,
    key: FrameKey,
    frame: Option<crate::ui::media::model::Frame>,
}
struct State {
    entries: HashMap<String, Entry>,
    waiters: HashMap<String, u64>,
    delivery: Delivery<Completion>,
    mailbox: aqua::mailbox::Mailbox<()>,
    next_generation: u64,
}
impl Drop for State {
    fn drop(&mut self) {
        for entry in self.entries.values() {
            if let Some((_, task)) = &entry.rendering {
                task.abort();
            }
        }
    }
}
#[derive(Clone)]
pub(crate) struct ModelState(Rc<RefCell<State>>);
impl Default for ModelState {
    fn default() -> Self {
        let mailbox = aqua::mailbox::Mailbox::new(NonZeroUsize::MIN);
        let delivery = Delivery::with_wake(NonZeroUsize::new(16).unwrap(), mailbox.sender());
        Self(Rc::new(RefCell::new(State {
            entries: HashMap::new(),
            waiters: HashMap::new(),
            delivery,
            mailbox,
            next_generation: 0,
        })))
    }
}
impl ModelState {
    pub(crate) fn bind(&self, ctx: &aqua::AppCtx<'_>, window: aqua::prelude::AquaWindowId) {
        ctx.bind_mailbox(window, &self.0.borrow().mailbox);
    }
    pub(crate) fn release(&self, id: &str) {
        self.suspend(id);
        self.0.borrow_mut().waiters.remove(id);
    }
    fn suspend(&self, id: &str) {
        if let Some(entry) = self.0.borrow_mut().entries.remove(id) {
            if let Some((_, task)) = entry.rendering {
                task.abort();
            }
        }
    }
    fn waiter(&self, id: &str) -> u64 {
        *self
            .0
            .borrow_mut()
            .waiters
            .entry(id.to_owned())
            .or_insert_with(|| NEXT_WAITER.fetch_add(1, Ordering::Relaxed))
    }
    pub(crate) fn drain(&self, images: &ImageAssets) -> bool {
        let mut state = self.0.borrow_mut();
        let mut changed = !state.mailbox.drain().is_empty();
        for completed in state.delivery.drain() {
            let Some(entry) = state.entries.get_mut(&completed.id) else {
                continue;
            };
            if entry.generation != completed.generation
                || !entry
                    .rendering
                    .as_ref()
                    .is_some_and(|(key, _)| *key == completed.key)
            {
                continue;
            }
            entry.rendering = None;
            if let Some(frame) = completed.frame
                && let Ok(decoded) = aqua::images::DecodedImage::from_rgba(
                    frame.width,
                    frame.height,
                    frame.pixels,
                )
            {
                if let Ok(image) = images.insert(decoded) {
                    entry.shown = Some((image, completed.key));
                }
            }
            changed = true;
        }
        changed
    }
    fn ensure(&self, id: &str, initial: Camera, shading: Shading) {
        let mut state = self.0.borrow_mut();
        if !state.entries.contains_key(id) {
            let generation = state.next_generation;
            state.next_generation = state.next_generation.wrapping_add(1);
            state.entries.insert(
                id.to_owned(),
                Entry {
                    owner: None,
                    model: None,
                    initial,
                    camera: initial,
                    shading,
                    drag: None,
                    bounds: Rc::new(Cell::new(None)),
                    shown: None,
                    rendering: None,
                    generation,
                },
            );
        }
    }
    fn ready(
        &self,
        id: &str,
        initial: Camera,
        shading: Shading,
        owner: Option<(String, Arc<[u64]>)>,
        model: usize,
    ) {
        let owner_changed = self
            .0
            .borrow()
            .entries
            .get(id)
            .is_some_and(|entry| entry.owner != owner);
        if owner_changed {
            self.suspend(id);
        }
        self.ensure(id, initial, shading);
        let mut state = self.0.borrow_mut();
        let changed = state.entries[id]
            .model
            .is_some_and(|previous| previous != model);
        if changed {
            let generation = state.next_generation;
            state.next_generation = state.next_generation.wrapping_add(1);
            let entry = state.entries.get_mut(id).unwrap();
            if let Some((_, task)) = entry.rendering.take() {
                task.abort();
            }
            entry.shown = None;
            entry.generation = generation;
        }
        let entry = state.entries.get_mut(id).unwrap();
        entry.owner = owner;
        entry.model = Some(model);
    }
    fn render_software(&self, id: &str, key: FrameKey, model: Arc<LoadedModel>) {
        let mut state = self.0.borrow_mut();
        let sender = state.delivery.sender();
        let Some(entry) = state.entries.get_mut(id) else {
            return;
        };
        if entry.rendering.is_some() || entry.shown.as_ref().is_some_and(|(_, shown)| *shown == key)
        {
            return;
        }
        let generation = entry.generation;
        let id = id.to_owned();
        let task = runtime::runtime().spawn(async move {
            let rendered = tokio::task::spawn_blocking(move || {
                let Geometry::Software(mesh) = &model.geometry else {
                    return None;
                };
                Some(render(
                    mesh,
                    &key.camera,
                    &key.look,
                    key.width,
                    key.height,
                    key.samples,
                ))
            })
            .await
            .ok()
            .flatten();
            let _ = sender
                .send(Completion {
                    id,
                    generation,
                    key,
                    frame: rendered,
                })
                .await;
        });
        entry.rendering = Some((key, task.abort_handle()));
    }
}

impl Entry {
    fn reset(&mut self) {
        self.camera = self.initial;
        self.drag = None;
    }
    fn orbit(&mut self, yaw: f32, pitch: f32) {
        self.camera.yaw = (self.camera.yaw + yaw).rem_euclid(360.0);
        self.camera.pitch = (self.camera.pitch + pitch).clamp(-89.0, 89.0);
    }
    fn zoom(&mut self, factor: f32) {
        self.camera.distance =
            (self.camera.distance * factor).clamp(Camera::NEAREST, Camera::FARTHEST);
    }
    fn key(&mut self, key: Key, modifiers: Modifiers) -> bool {
        let step = if modifiers.contains(Modifiers::SHIFT) {
            2.0
        } else {
            10.0
        };
        match key {
            Key::Named(NamedKey::Left) => self.orbit(-step, 0.0),
            Key::Named(NamedKey::Right) => self.orbit(step, 0.0),
            Key::Named(NamedKey::Up) => self.orbit(0.0, -step),
            Key::Named(NamedKey::Down) => self.orbit(0.0, step),
            Key::Char('=' | '+') => self.zoom(0.85),
            Key::Char('-') => self.zoom(1.0 / 0.85),
            Key::Char('0') => self.reset(),
            _ => return false,
        }
        true
    }
    fn move_to(&mut self, point: aqua_core::Point) -> bool {
        let Some(drag) = &mut self.drag else {
            return false;
        };
        let dx = point.x - drag.last.x;
        let dy = point.y - drag.last.y;
        drag.last = point;
        if drag.pan {
            let bounds = self.bounds.get().unwrap_or(Rect {
                x: 0.0,
                y: 0.0,
                w: 1.0,
                h: 1.0,
            });
            let per_pixel =
                self.camera.distance / Camera::focal(bounds.w.max(1.0), bounds.h.max(1.0));
            self.camera.pan.0 = dx.mul_add(-per_pixel, self.camera.pan.0).clamp(-1.5, 1.5);
            self.camera.pan.1 = dy.mul_add(per_pixel, self.camera.pan.1).clamp(-1.5, 1.5);
        } else {
            self.orbit(dx * 0.45, dy * 0.45);
        }
        true
    }
}

impl ModelBlock {
    pub(crate) fn view(
        &self,
        id: &str,
        width: f32,
        scale: f32,
        fetcher: Option<&MediaFetcher>,
        state: ModelState,
        services: &ModelServices,
    ) -> Element {
        let aspect = match (self.hint.width, self.hint.height) {
            (Some(w), Some(h)) if w > 0.0 && h > 0.0 => (w / h).clamp(0.5, 3.0),
            _ => 1.6,
        };
        let width = width.max(0.0).min(480.0 * aspect);
        let height = width / aspect;
        let initial = Camera::new(
            self.yaw.unwrap_or(Camera::DEFAULT_YAW),
            self.pitch.unwrap_or(Camera::DEFAULT_PITCH),
        );
        let waiter = state.waiter(id);
        let sender = state.0.borrow().mailbox.sender();
        let wake: aqua::mailbox::Wake = Arc::new(move || {
            let _ = sender.send(());
        });
        let loaded = (services.capability)().and_then(|capability| {
            services.assets.request(
                fetcher,
                &self.src,
                id,
                self.format,
                self.up,
                capability,
                waiter,
                wake,
            )
        });
        let model = match loaded {
            None => {
                state.suspend(id);
                return (services.frame)(
                    (services.placeholder)("Loading model…", false),
                    width,
                    aspect,
                    &self.display_name(),
                    None,
                );
            }
            Some(Err(reason)) => {
                state.suspend(id);
                return (services.frame)(
                    (services.placeholder)(&reason, true),
                    width,
                    aspect,
                    &self.display_name(),
                    None,
                );
            }
            Some(Ok(model)) => model,
        };
        let owner =
            fetcher.map(|fetcher| (fetcher.scope().to_owned(), Arc::clone(fetcher.owner())));
        state.ready(id, initial, self.shading, owner, Arc::as_ptr(&model).addr());
        let (camera, shading, dragging, moved, bounds, shown) = {
            let state = state.0.borrow();
            let entry = &state.entries[id];
            (
                entry.camera,
                entry.shading,
                entry.drag.is_some(),
                entry.camera != entry.initial,
                entry.bounds.clone(),
                entry.shown.as_ref().map(|(image, _)| image.clone()),
            )
        };
        let base = self.color.unwrap_or(glam::Vec3::new(0.52, 0.5, 0.48));
        let edges =
            (shading == Shading::Wireframe).then_some((glam::Vec3::new(0.01, 0.01, 0.012), 0.6));
        let look = Look { base, edges };
        let drawing = match &model.geometry {
            Geometry::Mesh(mesh) => {
                let mesh = mesh.clone();
                let measured = bounds.clone();
                ui::Canvas::new()
                    .style(
                        Style::new()
                            .width(Length::Px(width))
                            .height(Length::Px(height)),
                    )
                    .on_paint(move |bounds| {
                        measured.set(Some(bounds));
                        vec![DisplayItem::GpuiMesh {
                            bounds,
                            corners: CornerRadii::uniform(
                                (crate::ui::theme::RadiusTokens::value(
                                    crate::ui::theme::RadiusStep::Lg,
                                ) - 1.0)
                                    .max(0.0),
                            ),
                            mesh: mesh.clone(),
                            style: GpuiMeshStyle {
                                view: camera.view_matrix(),
                                projection: Camera::projection_matrix(
                                    bounds.w.max(1.0),
                                    bounds.h.max(1.0),
                                ),
                                base_color: base.to_array(),
                                edge_color: edges.map_or([0.0; 4], |(color, alpha)| {
                                    [color.x, color.y, color.z, alpha]
                                }),
                            },
                        }]
                    })
                    .build()
            }
            Geometry::Software(_) => {
                let side = |value: f32| (value * scale).round().clamp(1.0, 2400.0) as u32;
                let (pixel_width, pixel_height) = (side(width), side(height));
                let samples = if dragging || pixel_width * pixel_height * 4 > 3_000_000 {
                    1
                } else {
                    2
                };
                let key = FrameKey {
                    model: Arc::as_ptr(&model).addr(),
                    camera,
                    look,
                    width: pixel_width,
                    height: pixel_height,
                    samples,
                };
                state.render_software(id, key, model.clone());
                let measured = bounds.clone();
                let probe = ui::Canvas::new()
                    .style(
                        Style::new()
                            .width(Length::Px(width))
                            .height(Length::Px(height)),
                    )
                    .on_paint(move |bounds| {
                        measured.set(Some(bounds));
                        Vec::new()
                    })
                    .build();
                let mut layer = ui::Box::new().width(width).height(height).child(probe);
                if let Some(image) = shown {
                    layer = layer.child(
                        Media::new(image)
                            .width(width)
                            .height(height)
                            .fit(aqua_core::image::ImageFit::Contain)
                            .style(Style::new().absolute_at(Some(0.0), Some(0.0), None, None))
                            .build(),
                    );
                }
                layer.build()
            }
        };
        let down_state = state.clone();
        let down_id = id.to_owned();
        let down_invalidate = services.invalidate.clone();
        let copy = services.copy.clone();
        let source = self.src.raw.clone();
        let move_state = state.clone();
        let move_id = id.to_owned();
        let move_invalidate = services.invalidate.clone();
        let up_state = state.clone();
        let up_id = id.to_owned();
        let up_invalidate = services.invalidate.clone();
        let cancel_state = state.clone();
        let cancel_id = id.to_owned();
        let cancel_invalidate = services.invalidate.clone();
        let wheel_state = state.clone();
        let wheel_id = id.to_owned();
        let wheel_invalidate = services.invalidate.clone();
        let key_state = state.clone();
        let key_id = id.to_owned();
        let key_invalidate = services.invalidate.clone();
        let canvas = drawing
            .with_key(format!("{id}-frame"))
            .focusable(true)
            .aria_label(self.display_name())
            .cursor(if dragging {
                aqua_core::CursorIcon::Grabbing
            } else {
                aqua_core::CursorIcon::Grab
            })
            .on_input(InputKind::PointerDown, EventPhase::Bubble, move |event| {
                let RoutedInput::Native(InputEvent::PointerDown(pointer, button)) = &event.input
                else {
                    return InputResponse::default();
                };
                if *button == PointerButton::Secondary {
                    copy(pointer.position, &source);
                    return InputResponse {
                        stop_propagation: true,
                        ..InputResponse::handled()
                    };
                }
                if !matches!(button, PointerButton::Primary | PointerButton::Middle) {
                    return InputResponse::default();
                }
                if let Some(entry) = down_state.0.borrow_mut().entries.get_mut(&down_id) {
                    if *button == PointerButton::Primary && event.click_count.unwrap_or(1) >= 2 {
                        entry.reset();
                    } else {
                        entry.drag = Some(Drag {
                            last: pointer.position,
                            pan: *button == PointerButton::Middle
                                || event.modifiers.contains(Modifiers::SHIFT),
                        });
                    }
                }
                down_invalidate();
                InputResponse {
                    stop_propagation: true,
                    focus: FocusRequest::Focus,
                    pointer_capture: CaptureRequest::Capture,
                    ..InputResponse::default()
                }
            })
            .on_input(InputKind::PointerMove, EventPhase::Bubble, move |event| {
                let RoutedInput::Native(InputEvent::PointerMove(pointer)) = &event.input else {
                    return InputResponse::default();
                };
                if move_state
                    .0
                    .borrow_mut()
                    .entries
                    .get_mut(&move_id)
                    .is_some_and(|entry| entry.move_to(pointer.position))
                {
                    move_invalidate();
                }
                InputResponse::default()
            })
            .on_input(InputKind::PointerUp, EventPhase::Bubble, move |_| {
                if up_state
                    .0
                    .borrow_mut()
                    .entries
                    .get_mut(&up_id)
                    .is_some_and(|entry| entry.drag.take().is_some())
                {
                    up_invalidate();
                }
                InputResponse {
                    pointer_capture: CaptureRequest::Release,
                    ..InputResponse::default()
                }
            })
            .on_input(InputKind::PointerCancel, EventPhase::Bubble, move |_| {
                if cancel_state
                    .0
                    .borrow_mut()
                    .entries
                    .get_mut(&cancel_id)
                    .is_some_and(|entry| entry.drag.take().is_some())
                {
                    cancel_invalidate();
                }
                InputResponse {
                    pointer_capture: CaptureRequest::Release,
                    ..InputResponse::default()
                }
            })
            .on_input(InputKind::Scroll, EventPhase::Bubble, move |event| {
                let RoutedInput::Native(InputEvent::Scroll { delta, .. }) = &event.input else {
                    return InputResponse::default();
                };
                if !event.modifiers.contains(Modifiers::CONTROL)
                    && !event.modifiers.contains(Modifiers::LOGO)
                {
                    return InputResponse::default();
                }
                if let Some(entry) = wheel_state.0.borrow_mut().entries.get_mut(&wheel_id) {
                    entry.zoom((-delta.y * 0.0025).exp());
                }
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
                if !key_state
                    .0
                    .borrow_mut()
                    .entries
                    .get_mut(&key_id)
                    .is_some_and(|entry| entry.key(key.key, key.modifiers))
                {
                    return InputResponse::default();
                }
                key_invalidate();
                InputResponse {
                    stop_propagation: true,
                    ..InputResponse::handled()
                }
            });
        let shade_state = state.clone();
        let shade_id = id.to_owned();
        let shade_invalidate = services.invalidate.clone();
        let change_shading: Rc<dyn Fn(Shading)> = Rc::new(move |chosen| {
            if let Some(entry) = shade_state.0.borrow_mut().entries.get_mut(&shade_id) {
                entry.shading = chosen;
            }
            shade_invalidate();
        });
        let reset_state = state.clone();
        let reset_id = id.to_owned();
        let reset_invalidate = services.invalidate.clone();
        let reset: Rc<dyn Fn()> = Rc::new(move || {
            if let Some(entry) = reset_state.0.borrow_mut().entries.get_mut(&reset_id) {
                entry.reset();
            }
            reset_invalidate();
        });
        let noun = if model.triangles == 1 {
            "triangle"
        } else {
            "triangles"
        };
        let facts = format!(
            "{} · {} {noun}",
            model.format.label(),
            crate::ui::rich::kit::format_number(model.triangles as f64)
        );
        let frame = (services.frame)(canvas, width, aspect, &self.display_name(), Some(dragging));
        let toolbar = (services.toolbar)(shading, moved, &facts, change_shading, reset);
        ui::Box::new()
            .layout("column")
            .width(width)
            .gap(8.0)
            .child(frame)
            .child(toolbar)
            .build()
    }
}

#[cfg(test)]
#[path = "../../../../../tests/ui/rich/media/model/view.rs"]
mod tests;
