use std::{
    cell::Cell,
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
    element::{Box as UiBox, Element, Media, Text},
    input::{EventPhase, InputKind, InputResponse, RoutedInput},
    style::{Length, Style},
};

use super::{
    image::Images,
    source::{MediaFetcher, MediaSource},
};
use crate::{
    core::{Core, HostId, HostInstance},
    ui::{
        button::{Button, ButtonVariant},
        motion::MotionCurve,
        theme::{ArtisanTheme, Opacity},
    },
};

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
    SwipeStart(aqua_core::Point),
    SwipeMove(aqua_core::Point),
    SwipeEnd(aqua_core::Point),
    Backdrop,
    BackdropDown,
    CycleFocus(bool),
}

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
    swipe_start: Option<aqua_core::Point>,
    swipe_offset: f32,
    suppress_click: bool,
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
            swipe_start: None,
            swipe_offset: 0.0,
            suppress_click: false,
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
            swipe_start: None,
            swipe_offset: 0.0,
            suppress_click: false,
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
            Action::BackdropDown => {
                self.suppress_click = false;
                false
            }
            Action::Backdrop => {
                if std::mem::take(&mut self.suppress_click) {
                    false
                } else {
                    self.closing = Some(now);
                    true
                }
            }
            Action::Navigate(forward) => self.navigate(forward, now, reduced_motion),
            Action::SwipeStart(point) if self.gallery_count() > 1 => {
                self.swipe_start = Some(point);
                self.swipe_offset = 0.0;
                self.suppress_click = false;
                false
            }
            Action::SwipeMove(point) => {
                let Some(start) = self.swipe_start else {
                    return false;
                };
                let offset = point.x - start.x;
                if self.swipe_offset == offset {
                    false
                } else {
                    self.swipe_offset = offset;
                    true
                }
            }
            Action::SwipeEnd(point) => {
                let Some(start) = self.swipe_start.take() else {
                    return false;
                };
                let dx = point.x - start.x;
                let dy = point.y - start.y;
                let moved = self.swipe_offset != 0.0;
                self.swipe_offset = 0.0;
                self.suppress_click = dx.abs() > 8.0 || dy.abs() > 8.0;
                if dx.abs() >= 48.0 && dx.abs() > dy.abs() * 1.25 {
                    self.navigate(dx < 0.0, now, reduced_motion) || moved
                } else {
                    moved
                }
            }
            Action::SwipeStart(_) | Action::CycleFocus(_) => false,
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
        let (width, height) = self.picture_size(viewport);
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
                .absolute_at(
                    Some((viewport.w - width) / 2.0 + self.swipe_offset),
                    Some((viewport.h - height) / 2.0),
                    None,
                    None,
                )
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
                        .radius(5.0)
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
                    Text::new(if self.failed {
                        "Couldn't load image"
                    } else {
                        "Loading image…"
                    })
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
                                .radius(5.0)
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
        stage = stage.child(
            UiBox::new()
                .style(Style::new().absolute_at(None, Some(2.0), Some(2.0), None))
                .child(
                    Button::corner_close("image-preview-close", theme, "Close image preview", 5.0)
                        .on_activate(move |_| close(Action::Close))
                        .build(),
                )
                .build(),
        );
        let down = Rc::clone(&emit);
        let stage_release = Rc::clone(&emit);
        let stage = stage
            .build()
            .on_input(InputKind::PointerDown, EventPhase::Bubble, move |event| {
                if let RoutedInput::Native(InputEvent::PointerDown(
                    pointer,
                    PointerButton::Primary,
                )) = &event.input
                {
                    down(Action::SwipeStart(pointer.position));
                    return InputResponse {
                        stop_propagation: true,
                        ..InputResponse::capture()
                    };
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
                    stage_release(Action::SwipeEnd(pointer.position));
                }
                InputResponse {
                    stop_propagation: true,
                    pointer_capture: aqua_ui::input::CaptureRequest::Release,
                    ..InputResponse::default()
                }
            });
        let backdrop_pressed = Rc::new(Cell::new(None));
        let backdrop_press = Rc::clone(&backdrop_pressed);
        let backdrop_down = Rc::clone(&emit);
        let backdrop = Rc::clone(&emit);
        let moving = Rc::clone(&emit);
        let releasing = Rc::clone(&emit);
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
            .child(stage);
        if self.gallery_count() > 1 {
            for forward in [false, true] {
                let nav = Rc::clone(&emit);
                let button = Button::icon(
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
                )
                .variant(ButtonVariant::Ghost)
                .glass()
                .compact(36.0)
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
                            if forward { None } else { Some(8.0) },
                            Some(viewport.h / 2.0 - 18.0),
                            if forward { Some(8.0) } else { None },
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
                            .absolute_at(Some(0.0), None, None, Some(12.0))
                            .justify(aqua_ui::JustifyContent::CENTER),
                    )
                    .child(
                        Text::new(format!(
                            "{} / {}",
                            self.gallery_index() + 1,
                            self.gallery_count()
                        ))
                        .gpui_source_color(theme.colors.text_muted.to_reference_text_color())
                        .build(),
                    )
                    .build(),
            );
        }
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
                {
                    backdrop_press.set(Some(pointer.contact));
                    backdrop_down(Action::BackdropDown);
                }
                InputResponse::default()
            })
            .on_input(InputKind::PointerUp, EventPhase::Bubble, move |event| {
                if let RoutedInput::Native(InputEvent::PointerUp(pointer, PointerButton::Primary)) =
                    &event.input
                {
                    releasing(Action::SwipeEnd(pointer.position));
                    if backdrop_pressed.take() == Some(pointer.contact)
                        && event.bounds.contains(pointer.position)
                    {
                        backdrop(Action::Backdrop);
                    }
                }
                InputResponse::default()
            })
            .on_input(InputKind::PointerMove, EventPhase::Bubble, move |event| {
                if let RoutedInput::Native(InputEvent::PointerMove(pointer)) = &event.input {
                    moving(Action::SwipeMove(pointer.position));
                }
                InputResponse::default()
            })
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

#[cfg(all(test, feature = "test-support"))]
#[path = "../../../tests/ui/media/preview.rs"]
mod tests;
