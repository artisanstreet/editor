use std::{
    num::NonZeroUsize,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use aqua::images::ImageHandle;
use aqua_core::{InputEvent, Key, NamedKey, PointerButton, Size};
use aqua_ui::{
    element::{Box as UiBox, Element, Media},
    input::{EventPhase, InputKind, InputResponse, RoutedInput},
    style::{Length, Style},
};

use super::{
    image::Images,
    source::{MediaFetcher, MediaSource},
};
use editor_core::{Core, HostId, HostInstance};
use crate::ui::{
    kit::{
        Finish, IconButton, IconButtonSize, Media as MediaKind, Space, Surface, TextRole,
        control::icon_button::CORNER_INSET,
    },
    motion::MotionCurve,
    theme::{ArtisanTheme, Opacity},
};

const FRAME: Surface = Surface::Media(MediaKind::Frame);

static NEXT_WAITER: AtomicU64 = AtomicU64::new(1 << 62);

#[derive(Clone)]
pub struct OpenPreview {
    pub fetcher: MediaFetcher,
    pub sources: Vec<MediaSource>,
    pub index: usize,
    pub image: ImageHandle,
    pub opener_key: String,
}

#[derive(Clone)]
pub enum Effect {
    Open(OpenPreview),
    ShowCopyMenu(crate::ui::rich::actions::ShowCopyMenu),
    Clipboard(String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    Close,
    Navigate(bool),
    Press { at: aqua_core::Point, picture: bool },
    Drag { at: aqua_core::Point, viewport: Size },
    Release { at: aqua_core::Point },
    Zoom { at: aqua_core::Point, delta: f32, viewport: Size },
    CycleFocus(bool),
}

#[derive(Clone, Copy)]
enum Press {
    Backdrop { at: aqua_core::Point },
    Picture { at: aqua_core::Point, pan: aqua_core::Point },
}

const MAX_ZOOM: f32 = 8.0;
const ZOOM_PER_NOTCH: f32 = 1.2;
const NOTCH: f32 = 16.0;
const CLICK_SLOP: f32 = 8.0;

enum Content {
    ReadyOnly,
    Gallery {
        fetcher: MediaFetcher,
        loader: Images,
        sources: Vec<MediaSource>,
        index: usize,
    },
}

pub struct Preview {
    generation: u64,
    opener_key: Option<String>,
    host: HostId,
    instance: HostInstance,
    content: Content,
    image: Option<ImageHandle>,
    failed: bool,
    outgoing: Option<(ImageHandle, Instant, bool)>,
    started: Instant,
    slide_started: Option<Instant>,
    closing: Option<Instant>,
    forward: bool,
    press: Option<Press>,
    swipe_offset: f32,
    zoom: f32,
    pan: aqua_core::Point,
    waiter: u64,
    wake: aqua::mailbox::Mailbox<()>,
}

impl Preview {
    pub fn new(
        generation: u64,
        host: HostId,
        instance: HostInstance,
        fetcher: MediaFetcher,
        loader: Images,
        sources: Vec<MediaSource>,
        index: usize,
        image: ImageHandle,
        now: Instant,
    ) -> Option<Self> {
        (index < sources.len()).then(|| Self {
            generation,
            opener_key: None,
            host,
            instance,
            content: Content::Gallery {
                fetcher,
                loader,
                sources,
                index,
            },
            image: Some(image),
            failed: false,
            outgoing: None,
            started: now,
            slide_started: None,
            closing: None,
            forward: true,
            press: None,
            swipe_offset: 0.0,
            zoom: 1.0,
            pan: aqua_core::Point { x: 0.0, y: 0.0 },
            waiter: NEXT_WAITER.fetch_add(1, Ordering::Relaxed),
            wake: aqua::mailbox::Mailbox::new(NonZeroUsize::MIN),
        })
    }

    pub fn from_ready(
        generation: u64,
        host: HostId,
        instance: HostInstance,
        image: ImageHandle,
        opener_key: String,
        now: Instant,
    ) -> Self {
        Self {
            generation,
            opener_key: Some(opener_key),
            host,
            instance,
            content: Content::ReadyOnly,
            image: Some(image),
            failed: false,
            outgoing: None,
            started: now,
            slide_started: None,
            closing: None,
            forward: true,
            press: None,
            swipe_offset: 0.0,
            zoom: 1.0,
            pan: aqua_core::Point { x: 0.0, y: 0.0 },
            waiter: NEXT_WAITER.fetch_add(1, Ordering::Relaxed),
            wake: aqua::mailbox::Mailbox::new(NonZeroUsize::MIN),
        }
    }

    fn gallery_count(&self) -> usize {
        match &self.content {
            Content::ReadyOnly => 1,
            Content::Gallery { sources, .. } => sources.len(),
        }
    }

    fn gallery_index(&self) -> usize {
        match &self.content {
            Content::ReadyOnly => 0,
            Content::Gallery { index, .. } => *index,
        }
    }

    pub fn from_request(
        generation: u64,
        host: HostId,
        instance: HostInstance,
        loader: Images,
        request: OpenPreview,
        now: Instant,
    ) -> Option<Self> {
        let mut preview = Self::new(
            generation,
            host,
            instance,
            request.fetcher,
            loader,
            request.sources,
            request.index,
            request.image,
            now,
        )?;
        preview.opener_key = Some(request.opener_key);
        Some(preview)
    }

    pub fn opener_key(&self) -> Option<&str> {
        self.opener_key.as_deref()
    }

    pub fn wake_mailbox(&self) -> &aqua::mailbox::Mailbox<()> {
        &self.wake
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn is_live(&self, core: &Core) -> bool {
        core.host_if_instance(&self.host, self.instance).is_some()
    }

    pub fn refresh(&mut self, core: &Core) -> bool {
        self.wake.drain();
        if !self.is_live(core) || self.closing.is_some() || self.image.is_some() {
            return false;
        }
        let sender = self.wake.sender();
        let wake: aqua::mailbox::Wake = Arc::new(move || {
            let _ = sender.send(());
        });
        let Content::Gallery {
            fetcher,
            loader,
            sources,
            index,
        } = &self.content
        else {
            return false;
        };
        match loader.request(
            fetcher,
            &sources[*index],
            &format!("image-{index}"),
            self.waiter,
            wake,
        ) {
            Some(Ok(image)) => {
                self.image = Some(image);
                self.failed = false;
                true
            }
            Some(Err(_)) if !self.failed => {
                self.failed = true;
                true
            }
            _ => false,
        }
    }

    pub fn dispatch(
        &mut self,
        generation: u64,
        action: Action,
        core: &Core,
        now: Instant,
        reduced_motion: bool,
    ) -> bool {
        if generation != self.generation || !self.is_live(core) || self.closing.is_some() {
            return false;
        }
        match action {
            Action::Close => {
                self.closing = Some(now);
                true
            }
            Action::Navigate(forward) => self.navigate(forward, now, reduced_motion),
            Action::Press { at, picture } => {
                self.press = Some(if picture {
                    Press::Picture { at, pan: self.pan }
                } else {
                    Press::Backdrop { at }
                });
                self.swipe_offset = 0.0;
                false
            }
            Action::Drag { at, viewport } => match self.press {
                Some(Press::Picture { at: start, pan }) if self.zoom > 1.0 => {
                    let next = self.clamped(
                        aqua_core::Point {
                            x: pan.x + at.x - start.x,
                            y: pan.y + at.y - start.y,
                        },
                        viewport,
                    );
                    std::mem::replace(&mut self.pan, next) != next
                }
                Some(Press::Picture { at: start, .. }) if self.gallery_count() > 1 => {
                    let offset = at.x - start.x;
                    std::mem::replace(&mut self.swipe_offset, offset) != offset
                }
                Some(Press::Picture { .. } | Press::Backdrop { .. }) | None => false,
            },
            Action::Release { at } => match self.press.take() {
                Some(Press::Backdrop { at: start }) => {
                    let still = (at.x - start.x).abs() <= CLICK_SLOP
                        && (at.y - start.y).abs() <= CLICK_SLOP;
                    if still {
                        self.closing = Some(now);
                    }
                    still
                }
                Some(Press::Picture { at: start, .. }) => {
                    let dx = at.x - start.x;
                    let dy = at.y - start.y;
                    let moved = std::mem::replace(&mut self.swipe_offset, 0.0) != 0.0;
                    if self.zoom == 1.0 && dx.abs() >= 48.0 && dx.abs() > dy.abs() * 1.25 {
                        self.navigate(dx < 0.0, now, reduced_motion) || moved
                    } else {
                        moved
                    }
                }
                None => false,
            },
            Action::Zoom {
                at,
                delta,
                viewport,
            } => {
                let zoom = (self.zoom * ZOOM_PER_NOTCH.powf(-delta / NOTCH)).clamp(1.0, MAX_ZOOM);
                if zoom == self.zoom {
                    return false;
                }
                let ratio = zoom / self.zoom;
                let centre = aqua_core::Point {
                    x: viewport.w / 2.0 + self.pan.x,
                    y: viewport.h / 2.0 + self.pan.y,
                };
                self.zoom = zoom;
                self.pan = self.clamped(
                    aqua_core::Point {
                        x: self.pan.x + (at.x - centre.x) * (1.0 - ratio),
                        y: self.pan.y + (at.y - centre.y) * (1.0 - ratio),
                    },
                    viewport,
                );
                true
            }
            Action::CycleFocus(_) => false,
        }
    }

    fn clamped(&self, pan: aqua_core::Point, viewport: Size) -> aqua_core::Point {
        let (width, height) = self.picture_size(viewport);
        let reach = |picture: f32, room: f32| ((picture * self.zoom - room) / 2.0).max(0.0);
        let (x, y) = (reach(width, viewport.w), reach(height, viewport.h));
        aqua_core::Point {
            x: pan.x.clamp(-x, x),
            y: pan.y.clamp(-y, y),
        }
    }

    fn navigate(&mut self, forward: bool, now: Instant, reduced_motion: bool) -> bool {
        let Content::Gallery { sources, index, .. } = &mut self.content else {
            return false;
        };
        let next = if forward {
            index.saturating_add(1).min(sources.len() - 1)
        } else {
            index.saturating_sub(1)
        };
        if next == *index {
            return false;
        }
        *index = next;
        self.forward = forward;
        self.zoom = 1.0;
        self.pan = aqua_core::Point { x: 0.0, y: 0.0 };
        self.outgoing = if reduced_motion {
            self.image.take();
            None
        } else {
            self.image.take().map(|image| (image, now, forward))
        };
        self.failed = false;
        self.slide_started = Some(now);
        true
    }

    pub fn advance(&mut self, now: Instant, reduced_motion: bool) {
        if self.outgoing.as_ref().is_some_and(|(_, started, _)| {
            reduced_motion || now.saturating_duration_since(*started) >= Duration::from_millis(250)
        }) {
            self.outgoing = None;
        }
        if self.slide_started.is_some_and(|started| {
            reduced_motion || now.saturating_duration_since(started) >= Duration::from_millis(250)
        }) {
            self.slide_started = None;
        }
    }

    pub fn finished(&self, now: Instant, reduced_motion: bool) -> bool {
        self.closing.is_some_and(|start| {
            reduced_motion || now.saturating_duration_since(start) >= Duration::from_millis(250)
        })
    }

    pub fn animating(&self, now: Instant, reduced_motion: bool) -> bool {
        !reduced_motion
            && (self.closing.is_some_and(|start| {
                now.saturating_duration_since(start) < Duration::from_millis(250)
            }) || now.saturating_duration_since(self.started) < Duration::from_millis(250)
                || self.outgoing.as_ref().is_some_and(|(_, start, _)| {
                    now.saturating_duration_since(*start) < Duration::from_millis(250)
                }))
    }

    #[expect(
        clippy::cast_precision_loss,
        reason = "image dimensions are bounded by the decoder"
    )]
    fn picture_size(&self, viewport: Size) -> (f32, f32) {
        let Some(image) = &self.image else {
            return ((viewport.w - 112.0).max(1.0).min(320.0), 180.0);
        };
        let aqua::images::ImageStatus::Ready { width, height } = image.status() else {
            return ((viewport.w - 112.0).max(1.0).min(320.0), 180.0);
        };
        let width = width.max(1) as f32;
        let height = height.max(1) as f32;
        let scale = ((viewport.w - 112.0).max(1.0).min(960.0) / width)
            .min((viewport.h - 112.0).max(1.0).min(720.0) / height)
            .min(1.0);
        (width * scale, height * scale)
    }

    pub fn view(
        &self,
        theme: ArtisanTheme,
        viewport: Size,
        now: Instant,
        reduced_motion: bool,
        emit: Rc<dyn Fn(u64, Action)>,
    ) -> Element {
        let generation = self.generation;
        let emit: Rc<dyn Fn(Action)> = Rc::new(move |action| emit(generation, action));
        let (fitted_width, fitted_height) = self.picture_size(viewport);
        let (width, height) = (fitted_width * self.zoom, fitted_height * self.zoom);
        let left = (viewport.w - width) / 2.0 + self.pan.x + self.swipe_offset;
        let top = (viewport.h - height) / 2.0 + self.pan.y;
        let duration = 0.25;
        let progress = |since: Instant| {
            MotionCurve::SmoothOut.sample(f64::from(
                (now.saturating_duration_since(since).as_secs_f32() / duration).min(1.0),
            )) as f32
        };
        let presence = if reduced_motion {
            1.0
        } else if let Some(closing) = self.closing {
            1.0 - progress(closing)
        } else {
            progress(self.started)
        };
        let close = Rc::clone(&emit);
        let scale = 0.04_f32.mul_add(presence, 0.96);
        let transform = aqua_core::Transform2D {
            a: scale,
            b: 0.0,
            c: 0.0,
            d: scale,
            tx: (1.0 - scale) * width / 2.0,
            ty: (1.0 - scale) * height / 2.0,
        };
        let slide = self
            .slide_started
            .filter(|_| !reduced_motion)
            .map(progress)
            .unwrap_or(1.0);
        let mut stage = UiBox::new().width(width).height(height).style(
            Style::new()
                .absolute_at(Some(left), Some(top), None, None)
                .transform(transform),
        );
        stage = stage.child(if let Some(image) = &self.image {
            UiBox::new()
                .width(width)
                .height(height)
                .opacity(slide)
                .style(
                    Style::new()
                        .absolute_at(
                            Some(8.0 * if self.forward { 1.0 } else { -1.0 } * (1.0 - slide)),
                            Some(0.0),
                            None,
                            None,
                        )
                        .content_blur(3.0 * (1.0 - slide)),
                )
                .child(
                    UiBox::new()
                        .width(width)
                        .height(height)
                        .radius(FRAME.radius_px())
                        .clip(true)
                        .child(
                            Media::new(image.clone())
                                .width(width)
                                .height(height)
                                .fit(aqua_core::image::ImageFit::Contain)
                                .build(),
                        )
                        .build(),
                )
                .build()
        } else {
            UiBox::new()
                .width(width)
                .height(height)
                .style(
                    Style::new()
                        .align(aqua_ui::AlignItems::CENTER)
                        .justify(aqua_ui::JustifyContent::CENTER),
                )
                .child(
                    TextRole::Label
                        .text(
                            &theme,
                            if self.failed {
                                "Couldn't load image"
                            } else {
                                "Loading image…"
                            },
                        )
                    .gpui_source_color(theme.colors.text_muted.to_reference_text_color())
                    .build(),
                )
                .build()
        });
        if let Some((image, started, forward)) = &self.outgoing {
            let outgoing = if reduced_motion {
                1.0
            } else {
                progress(*started)
            };
            if outgoing < 1.0 {
                stage = stage.child(
                    UiBox::new()
                        .width(width)
                        .height(height)
                        .opacity(1.0 - outgoing)
                        .style(
                            Style::new()
                                .absolute_at(
                                    Some(8.0 * if *forward { -1.0 } else { 1.0 } * outgoing),
                                    Some(0.0),
                                    None,
                                    None,
                                )
                                .content_blur(3.0 * outgoing),
                        )
                        .child(
                            UiBox::new()
                                .width(width)
                                .height(height)
                                .radius(FRAME.radius_px())
                                .clip(true)
                                .child(
                                    Media::new(image.clone())
                                        .width(width)
                                        .height(height)
                                        .fit(aqua_core::image::ImageFit::Contain)
                                        .build(),
                                )
                                .build(),
                        )
                        .build(),
                );
            }
        }
        let pressing = Rc::clone(&emit);
        let stage = track(
            stage
                .build()
                .on_input(InputKind::PointerDown, EventPhase::Bubble, move |event| {
                    let primary = if let RoutedInput::Native(InputEvent::PointerDown(
                        pointer,
                        PointerButton::Primary,
                    )) = &event.input
                    {
                        pressing(Action::Press {
                            at: pointer.position,
                            picture: true,
                        });
                        true
                    } else {
                        false
                    };
                    InputResponse {
                        stop_propagation: true,
                        pointer_capture: if primary {
                            aqua_ui::input::CaptureRequest::Capture
                        } else {
                            aqua_ui::input::CaptureRequest::None
                        },
                        ..InputResponse::default()
                    }
                }),
            &emit,
            viewport,
        );
        let close_button = UiBox::new()
            .style(Style::new().absolute_at(
                None,
                Some(top.max(0.0) + CORNER_INSET),
                Some((viewport.w - (left + width)).max(0.0) + CORNER_INSET),
                None,
            ))
            .child(
                IconButton::corner_close(
                    "image-preview-close",
                    theme,
                    "Close image preview",
                    FRAME.radius_px(),
                )
                .on_activate(move |_| close(Action::Close))
                .build(),
            )
            .build();
        let backdrop = Rc::clone(&emit);
        let zooming = Rc::clone(&emit);
        let keys = Rc::clone(&emit);
        let mut root = UiBox::new()
            .width(viewport.w)
            .height(viewport.h)
            .opacity(presence)
            .style(
                Style::new()
                    .absolute_at(Some(0.0), Some(0.0), None, None)
                    .flex_shrink(0.0)
                    .gpui_background(aqua_core::GpuiBackground::Solid(
                        theme
                            .colors
                            .background
                            .at(Opacity::BackdropDense)
                            .to_reference_paint(),
                    )),
            )
            .child(stage)
            .child(close_button);
        if self.gallery_count() > 1 {
            for forward in [false, true] {
                let nav = Rc::clone(&emit);
                let nav_size = IconButtonSize::Lg;
                let button = IconButton::new(
                    if forward {
                        "image-preview-next"
                    } else {
                        "image-preview-previous"
                    },
                    theme,
                    if forward {
                        editor_assets::AssetId::TABLER_CHEVRON_RIGHT
                    } else {
                        editor_assets::AssetId::TABLER_CHEVRON_LEFT
                    },
                    if forward {
                        "Next image"
                    } else {
                        "Previous image"
                    },
                    nav_size,
                    Finish::Glass,
                )
                .disabled(if forward {
                    self.gallery_index() + 1 == self.gallery_count()
                } else {
                    self.gallery_index() == 0
                })
                .on_activate(move |_| nav(Action::Navigate(forward)))
                .build();
                root = root.child(
                    UiBox::new()
                        .style(Style::new().absolute_at(
                            if forward { None } else { Some(Space::S8.px()) },
                            Some((viewport.h - nav_size.edge()) / 2.0),
                            if forward { Some(Space::S8.px()) } else { None },
                            None,
                        ))
                        .child(button)
                        .build(),
                );
            }
            root = root.child(
                UiBox::new()
                    .width(Length::Pct(100.0))
                    .style(
                        Style::new()
                            .absolute_at(Some(0.0), None, None, Some(Space::S12.px()))
                            .justify(aqua_ui::JustifyContent::CENTER),
                    )
                    .child(
                        TextRole::Caption.text(
                            &theme,
                            format!("{} / {}", self.gallery_index() + 1, self.gallery_count()),
                        )
                        .gpui_source_color(theme.colors.text_muted.to_reference_text_color())
                        .build(),
                    )
                    .build(),
            );
        }
        track(
            root.build()
                .with_key("image-preview")
                .role(aqua_core::Role::Dialog)
                .aria_label("Image preview")
                .focusable(true)
                .on_input(InputKind::PointerDown, EventPhase::Bubble, move |event| {
                    if let RoutedInput::Native(InputEvent::PointerDown(
                        pointer,
                        PointerButton::Primary,
                    )) = &event.input
                        && event.target == event.current_target
                    {
                        backdrop(Action::Press {
                            at: pointer.position,
                            picture: false,
                        });
                        return InputResponse {
                            pointer_capture: aqua_ui::input::CaptureRequest::Capture,
                            ..InputResponse::default()
                        };
                    }
                    InputResponse::default()
                })
                .on_input(InputKind::Scroll, EventPhase::Bubble, move |event| {
                    if let RoutedInput::Native(InputEvent::Scroll { at, delta }) = &event.input {
                        zooming(Action::Zoom {
                            at: *at,
                            delta: delta.y,
                            viewport,
                        });
                    }
                    InputResponse {
                        stop_propagation: true,
                        ..InputResponse::handled()
                    }
                }),
            &emit,
            viewport,
        )
        .on_input(InputKind::KeyDown, EventPhase::Bubble, move |event| {
                if let RoutedInput::Native(InputEvent::KeyDown(key)) = &event.input {
                    let action = match key.key {
                        Key::Named(NamedKey::Escape) => Some(Action::Close),
                        Key::Named(NamedKey::Left) => Some(Action::Navigate(false)),
                        Key::Named(NamedKey::Right) => Some(Action::Navigate(true)),
                        Key::Named(NamedKey::Tab) => Some(Action::CycleFocus(
                            key.modifiers.contains(aqua_core::Modifiers::SHIFT),
                        )),
                        _ => None,
                    };
                    if let Some(action) = action {
                        keys(action);
                        return InputResponse {
                            stop_propagation: true,
                            ..InputResponse::handled()
                        };
                    }
                }
                InputResponse::default()
            })
    }
}

fn track(element: Element, emit: &Rc<dyn Fn(Action)>, viewport: Size) -> Element {
    let dragging = Rc::clone(emit);
    let releasing = Rc::clone(emit);
    element
        .on_input(InputKind::PointerMove, EventPhase::Bubble, move |event| {
            if let RoutedInput::Native(InputEvent::PointerMove(pointer)) = &event.input {
                dragging(Action::Drag {
                    at: pointer.position,
                    viewport,
                });
            }
            InputResponse {
                stop_propagation: true,
                ..InputResponse::default()
            }
        })
        .on_input(InputKind::PointerUp, EventPhase::Bubble, move |event| {
            if let RoutedInput::Native(InputEvent::PointerUp(pointer, PointerButton::Primary)) =
                &event.input
            {
                releasing(Action::Release {
                    at: pointer.position,
                });
            }
            InputResponse {
                stop_propagation: true,
                pointer_capture: aqua_ui::input::CaptureRequest::Release,
                ..InputResponse::default()
            }
        })
}

#[cfg(all(test, feature = "test-support"))]
#[path = "../../../tests/ui/media/preview.rs"]
mod tests;
