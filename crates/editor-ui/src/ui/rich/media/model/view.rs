use super::super::{menu_anchor, opens_context_menu};
use super::{ModelBlock, Shading};
use crate::ui::media::{
    model::{
        Camera, Cancelled, Look,
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
    tree::AccessibilityRequest,
};
use editor_core::{delivery::Delivery, runtime};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    num::NonZeroUsize,
    rc::Rc,
    sync::{
        Arc, LazyLock,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

static NEXT_WAITER: AtomicU64 = AtomicU64::new(1);
static RASTER_WORKERS: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(2)));

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
    pub focus_ring: Style,
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
    rendering: Option<Rendering>,
    generation: u64,
}
struct Rendering {
    key: FrameKey,
    stop: CancellationToken,
}
impl Drop for Rendering {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}
struct Completion {
    id: String,
    generation: u64,
    key: FrameKey,
    frame: crate::ui::media::model::Frame,
}
struct State {
    entries: HashMap<String, Entry>,
    waiters: HashMap<String, u64>,
    delivery: Delivery<Completion>,
    mailbox: aqua::mailbox::Mailbox<()>,
    next_generation: u64,
}
#[derive(Clone)]
pub(crate) struct ModelState(Rc<RefCell<State>>);
impl Default for ModelState {
    fn default() -> Self {
        let mailbox = aqua::mailbox::Mailbox::new(NonZeroUsize::MIN);
        let delivery = Delivery::with_wake(
            const { NonZeroUsize::new(16).expect("invariant: the delivery capacity 16 is nonzero") },
            mailbox.sender(),
        );
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
        let removed = self.0.borrow_mut().entries.remove(id);
        drop(removed);
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
                    .is_some_and(|rendering| rendering.key == completed.key)
            {
                continue;
            }
            entry.rendering = None;
            let frame = completed.frame;
            if let Ok(decoded) =
                aqua::images::DecodedImage::from_rgba(frame.width, frame.height, frame.pixels)
                && let Ok(image) = images.insert(decoded)
            {
                entry.shown = Some((image, completed.key));
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
            let entry = state
                .entries
                .get_mut(id)
                .unwrap_or_else(|| panic!("invariant: model {id} was ensured above"));
            entry.rendering = None;
            entry.shown = None;
            entry.generation = generation;
        }
        let entry = state
            .entries
            .get_mut(id)
            .unwrap_or_else(|| panic!("invariant: model {id} was ensured above"));
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
        let stop = CancellationToken::new();
        let task_stop = stop.clone();
        runtime::runtime().spawn(async move {
            let permit = tokio::select! {
                () = task_stop.cancelled() => return,
                permit = Arc::clone(&RASTER_WORKERS).acquire_owned() => {
                    permit.expect("invariant: the raster worker semaphore is never closed")
                }
            };
            let raster_stop = task_stop.clone();
            let rendered = tokio::task::spawn_blocking(move || {
                let _permit = permit;
                let Geometry::Software(mesh) = &model.geometry else {
                    unreachable!("invariant: software frames render software geometry");
                };
                render(
                    mesh,
                    &key.camera,
                    &key.look,
                    key.width,
                    key.height,
                    key.samples,
                    &|| raster_stop.is_cancelled(),
                )
            })
            .await;
            let frame = match rendered {
                Ok(Ok(frame)) => frame,
                Ok(Err(Cancelled)) => return,
                Err(error) if error.is_cancelled() => return,
                Err(error) => std::panic::resume_unwind(error.into_panic()),
            };
            sender
                .offer(Completion {
                    id,
                    generation,
                    key,
                    frame,
                })
                .await;
        });
        entry.rendering = Some(Rendering { key, stop });
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
            Key::Char(_) | Key::Named(_) => return false,
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

enum Loaded {
    Ready(Arc<LoadedModel>),
    Placeholder(Element),
}

struct EntryView {
    camera: Camera,
    shading: Shading,
    dragging: bool,
    moved: bool,
    bounds: Rc<Cell<Option<Rect>>>,
    shown: Option<ImageHandle>,
}

#[derive(Clone, Copy)]
struct CanvasSize {
    width: f32,
    height: f32,
    scale: f32,
}

fn on_entry<A: 'static>(
    state: &ModelState,
    id: &str,
    invalidate: &Rc<dyn Fn()>,
    change: impl Fn(&mut Entry, A) -> bool + 'static,
) -> impl Fn(A) -> bool + 'static {
    let state = state.clone();
    let id = id.to_owned();
    let invalidate = Rc::clone(invalidate);
    move |argument| {
        let changed = state
            .0
            .borrow_mut()
            .entries
            .get_mut(&id)
            .is_some_and(|entry| change(entry, argument));
        if changed {
            invalidate();
        }
        changed
    }
}

fn entry_view(state: &ModelState, id: &str) -> EntryView {
    let state = state.0.borrow();
    let entry = &state.entries[id];
    EntryView {
        camera: entry.camera,
        shading: entry.shading,
        dragging: entry.drag.is_some(),
        moved: entry.camera != entry.initial,
        bounds: Rc::clone(&entry.bounds),
        shown: entry.shown.as_ref().map(|(image, _)| image.clone()),
    }
}

fn mesh_canvas(
    mesh: &Arc<aqua_core::gpui_mesh::GpuiMesh>,
    entry: &EntryView,
    look: Look,
    size: CanvasSize,
) -> Element {
    let mesh = Arc::clone(mesh);
    let measured = Rc::clone(&entry.bounds);
    let camera = entry.camera;
    ui::Canvas::new()
        .style(
            Style::new()
                .width(Length::Px(size.width))
                .height(Length::Px(size.height)),
        )
        .on_paint(move |bounds| {
            measured.set(Some(bounds));
            vec![DisplayItem::GpuiMesh {
                bounds,
                corners: CornerRadii::uniform(
                    (crate::ui::theme::RadiusTokens::value(crate::ui::theme::RadiusStep::Lg) - 1.0)
                        .max(0.0),
                ),
                mesh: Arc::clone(&mesh),
                style: GpuiMeshStyle {
                    view: camera.view_matrix(),
                    projection: Camera::projection_matrix(bounds.w.max(1.0), bounds.h.max(1.0)),
                    base_color: look.base.to_array(),
                    edge_color: look.edges.map_or([0.0; 4], |(color, alpha)| {
                        [color.x, color.y, color.z, alpha]
                    }),
                },
            }]
        })
        .build()
}

fn software_canvas(
    id: &str,
    state: &ModelState,
    model: &Arc<LoadedModel>,
    entry: &EntryView,
    look: Look,
    size: CanvasSize,
) -> Element {
    let CanvasSize {
        width,
        height,
        scale,
    } = size;
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the side is clamped to 1..=2400 pixels first"
    )]
    let pixels = |value: f32| (value * scale).round().clamp(1.0, 2400.0) as u32;
    let (pixel_width, pixel_height) = (pixels(width), pixels(height));
    let samples = if entry.dragging || pixel_width * pixel_height * 4 > 3_000_000 {
        1
    } else {
        2
    };
    let key = FrameKey {
        model: Arc::as_ptr(model).addr(),
        camera: entry.camera,
        look,
        width: pixel_width,
        height: pixel_height,
        samples,
    };
    state.render_software(id, key, Arc::clone(model));
    let measured = Rc::clone(&entry.bounds);
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
    if let Some(image) = entry.shown.clone() {
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

impl ModelBlock {
    pub(crate) fn view(
        &self,
        id: &str,
        width: f32,
        scale: f32,
        fetcher: Option<&MediaFetcher>,
        state: &ModelState,
        services: &ModelServices,
    ) -> Element {
        let aspect = match (self.hint.width, self.hint.height) {
            (Some(w), Some(h)) if w > 0.0 && h > 0.0 => (w / h).clamp(0.5, 3.0),
            _ => 1.6,
        };
        let width = width.max(0.0).min(480.0 * aspect);
        let size = CanvasSize {
            width,
            height: width / aspect,
            scale,
        };
        let model = match self.load(id, fetcher, state, services) {
            Loaded::Ready(model) => model,
            Loaded::Placeholder(placeholder) => {
                state.suspend(id);
                return (services.frame)(placeholder, width, aspect, &self.display_name(), None);
            }
        };
        let initial = Camera::new(
            self.yaw.unwrap_or(Camera::DEFAULT_YAW),
            self.pitch.unwrap_or(Camera::DEFAULT_PITCH),
        );
        let owner =
            fetcher.map(|fetcher| (fetcher.scope().to_owned(), Arc::clone(fetcher.owner())));
        state.ready(id, initial, self.shading, owner, Arc::as_ptr(&model).addr());
        let entry = entry_view(state, id);
        let look = Look {
            base: self.color.unwrap_or(glam::Vec3::new(0.52, 0.5, 0.48)),
            edges: (entry.shading == Shading::Wireframe)
                .then_some((glam::Vec3::new(0.01, 0.01, 0.012), 0.6)),
        };
        let drawing = match &model.geometry {
            Geometry::Mesh(mesh) => mesh_canvas(mesh, &entry, look, size),
            Geometry::Software(_) => software_canvas(id, state, &model, &entry, look, size),
        };
        let canvas = self.interactive(drawing, id, state, services, &entry);
        let change_shading: Rc<dyn Fn(Shading)> = {
            let shade = on_entry(state, id, &services.invalidate, |entry, chosen| {
                entry.shading = chosen;
                true
            });
            Rc::new(move |chosen| {
                shade(chosen);
            })
        };
        let reset: Rc<dyn Fn()> = {
            let reset = on_entry(state, id, &services.invalidate, |entry, ()| {
                entry.reset();
                true
            });
            Rc::new(move || {
                reset(());
            })
        };
        let facts = format!(
            "{} · {}",
            model.format.label(),
            crate::ui::locale::count(model.triangles, "triangle", "triangles")
        );
        let frame = (services.frame)(
            canvas,
            width,
            aspect,
            &self.display_name(),
            Some(entry.dragging),
        );
        let toolbar = (services.toolbar)(entry.shading, entry.moved, &facts, change_shading, reset);
        ui::Box::new()
            .layout("column")
            .width(width)
            .gap(8.0)
            .child(frame)
            .child(toolbar)
            .build()
    }

    fn load(
        &self,
        id: &str,
        fetcher: Option<&MediaFetcher>,
        state: &ModelState,
        services: &ModelServices,
    ) -> Loaded {
        let waiter = state.waiter(id);
        let wake = crate::ui::media::wake_on(&state.0.borrow().mailbox);
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
        match loaded {
            None => Loaded::Placeholder((services.placeholder)("Loading model…", false)),
            Some(Err(reason)) => Loaded::Placeholder((services.placeholder)(&reason, true)),
            Some(Ok(model)) => Loaded::Ready(model),
        }
    }

    fn interactive(
        &self,
        drawing: Element,
        id: &str,
        state: &ModelState,
        services: &ModelServices,
        entry: &EntryView,
    ) -> Element {
        let element = drawing
            .with_key(format!("{id}-frame"))
            .focusable(true)
            .focus_visible_style(services.focus_ring.clone())
            .aria_label(self.display_name())
            .cursor(if entry.dragging {
                aqua_core::CursorIcon::Grabbing
            } else {
                aqua_core::CursorIcon::Grab
            });
        let element = self.pointer_input(element, id, state, services);
        self.menu_and_keys(element, id, state, services, &entry.bounds)
    }

    fn press_input(
        &self,
        element: Element,
        id: &str,
        state: &ModelState,
        services: &ModelServices,
    ) -> Element {
        let copy = Rc::clone(&services.copy);
        let source = self.src.raw().to_owned();
        let press =
            on_entry(
                state,
                id,
                &services.invalidate,
                |entry,
                 (position, button, double, shift): (
                    aqua_core::Point,
                    PointerButton,
                    bool,
                    bool,
                )| {
                    if button == PointerButton::Primary && double {
                        entry.reset();
                    } else {
                        entry.drag = Some(Drag {
                            last: position,
                            pan: button == PointerButton::Middle || shift,
                        });
                    }
                    true
                },
            );
        element.on_input(InputKind::PointerDown, EventPhase::Bubble, move |event| {
            let RoutedInput::Native(InputEvent::PointerDown(pointer, button)) = &event.input else {
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
            press((
                pointer.position,
                *button,
                event.click_count.unwrap_or(1) >= 2,
                event.modifiers.contains(Modifiers::SHIFT),
            ));
            InputResponse {
                stop_propagation: true,
                focus: FocusRequest::Focus,
                pointer_capture: CaptureRequest::Capture,
                ..InputResponse::default()
            }
        })
    }

    fn pointer_input(
        &self,
        element: Element,
        id: &str,
        state: &ModelState,
        services: &ModelServices,
    ) -> Element {
        let drag = on_entry(state, id, &services.invalidate, |entry, position| {
            entry.move_to(position)
        });
        let release = on_entry(state, id, &services.invalidate, |entry, ()| {
            entry.drag.take().is_some()
        });
        let cancel = on_entry(state, id, &services.invalidate, |entry, ()| {
            entry.drag.take().is_some()
        });
        let zoom = on_entry(state, id, &services.invalidate, |entry, factor: f32| {
            entry.zoom(factor);
            true
        });
        self.press_input(element, id, state, services)
            .on_input(InputKind::PointerMove, EventPhase::Bubble, move |event| {
                if let RoutedInput::Native(InputEvent::PointerMove(pointer)) = &event.input {
                    drag(pointer.position);
                }
                InputResponse::default()
            })
            .on_input(InputKind::PointerUp, EventPhase::Bubble, move |_| {
                release(());
                InputResponse {
                    pointer_capture: CaptureRequest::Release,
                    ..InputResponse::default()
                }
            })
            .on_input(InputKind::PointerCancel, EventPhase::Bubble, move |_| {
                cancel(());
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
                zoom((-delta.y * 0.0025).exp());
                InputResponse {
                    stop_propagation: true,
                    ..InputResponse::handled()
                }
            })
    }

    fn menu_and_keys(
        &self,
        element: Element,
        id: &str,
        state: &ModelState,
        services: &ModelServices,
        bounds: &Rc<Cell<Option<Rect>>>,
    ) -> Element {
        let key_copy = Rc::clone(&services.copy);
        let key_source = self.src.raw().to_owned();
        let request_copy = Rc::clone(&services.copy);
        let request_source = self.src.raw().to_owned();
        let request_bounds = Rc::clone(bounds);
        let steer = on_entry(
            state,
            id,
            &services.invalidate,
            |entry, (key, modifiers): (Key, Modifiers)| entry.key(key, modifiers),
        );
        element
            .on_accessibility_request(move |request| {
                if request == AccessibilityRequest::ShowContextMenu {
                    let anchor = request_bounds
                        .get()
                        .map_or(aqua_core::Point::ZERO, menu_anchor);
                    request_copy(anchor, &request_source);
                }
            })
            .on_input(InputKind::KeyDown, EventPhase::Bubble, move |event| {
                let RoutedInput::Native(InputEvent::KeyDown(key)) = &event.input else {
                    return InputResponse::default();
                };
                if opens_context_menu(key) {
                    key_copy(menu_anchor(event.bounds), &key_source);
                    return InputResponse {
                        stop_propagation: true,
                        ..InputResponse::handled()
                    };
                }
                if !steer((key.key, key.modifiers)) {
                    return InputResponse::default();
                }
                InputResponse {
                    stop_propagation: true,
                    ..InputResponse::handled()
                }
            })
    }
}

#[cfg(test)]
#[path = "../../../../../tests/ui/rich/media/model/view.rs"]
mod tests;
