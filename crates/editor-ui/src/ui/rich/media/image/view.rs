use crate::ui::kit::{
    Button, ButtonSize, ButtonTone, IconSize, Media as Framing, Space, Spaced, Surface, TextRole,
};
use std::{
    cell::Cell,
    num::NonZeroUsize,
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
};

use aqua::images::{ImageHandle, ImageStatus};
use aqua_core::{InputEvent, PointerButton};
use aqua_ui::{
    element::{Box as UiBox, Media, Text},
    input::{EventPhase, InputKind, InputResponse, RoutedInput},
    style::{Length, Style},
    tree::AccessibilityRequest,
};

use super::super::{menu_anchor, opens_context_menu};
use super::{ImageBlock, gallery_rows};
use crate::ui::{
    media::{
        image::{ImageError, ImageLoader, Images, cache_key},
        source::{MediaFetcher, MediaLoadError, MediaSource},
    },
    theme::ArtisanTheme,
};

static NEXT_WAITER: AtomicU64 = AtomicU64::new(1 << 63);

enum Loaded {
    Loading,
    Ready(ImageHandle),
    Failed(ImageError),
}

#[derive(Clone)]
pub(crate) enum Action {
    Open {
        index: usize,
        image: ImageHandle,
    },
    Retry(usize),
    AllowRemote,
    CopyPath {
        index: usize,
        position: aqua_core::Point,
    },
}

pub(crate) struct ImageState {
    loader: Images,
    fetcher: MediaFetcher,
    sources: Vec<MediaSource>,
    loaded: Vec<Loaded>,
    anchors: Vec<Rc<Cell<Option<aqua_core::Point>>>>,
    waiters: Vec<u64>,
    wake: aqua::mailbox::Mailbox<()>,
}

impl ImageState {
    pub(crate) fn new(block: &ImageBlock, fetcher: MediaFetcher, loader: Images) -> Self {
        let sources: Vec<_> = block.images.iter().map(|image| image.src.clone()).collect();
        let mut state = Self {
            loaded: sources.iter().map(|_| Loaded::Loading).collect(),
            anchors: sources.iter().map(|_| Rc::new(Cell::new(None))).collect(),
            waiters: sources
                .iter()
                .map(|_| NEXT_WAITER.fetch_add(1, Ordering::Relaxed))
                .collect(),
            sources,
            fetcher,
            loader,
            wake: aqua::mailbox::Mailbox::new(NonZeroUsize::MIN),
        };
        state.refresh();
        state
    }

    pub(crate) fn preview_sources(&self) -> (MediaFetcher, Vec<MediaSource>) {
        (self.fetcher.clone(), self.sources.clone())
    }

    pub(crate) fn wake_mailbox(&self) -> &aqua::mailbox::Mailbox<()> {
        &self.wake
    }

    pub(crate) fn refresh(&mut self) -> bool {
        self.wake.drain();
        let mut changed = false;
        for (index, source) in self.sources.iter().enumerate() {
            let wake = crate::ui::media::wake_on(&self.wake);
            let next = match self.loader.request(
                &self.fetcher,
                source,
                &format!("image-{index}"),
                self.waiters[index],
                wake,
            ) {
                None => Loaded::Loading,
                Some(Ok(image)) => Loaded::Ready(image),
                Some(Err(error)) => Loaded::Failed(error),
            };
            let same = match (&self.loaded[index], &next) {
                (Loaded::Loading, Loaded::Loading) => true,
                (Loaded::Ready(a), Loaded::Ready(b)) => a.id() == b.id(),
                (Loaded::Failed(a), Loaded::Failed(b)) => a == b,
                _ => false,
            };
            changed |= !same;
            self.loaded[index] = next;
        }
        changed
    }

    pub(crate) fn allow_remote(&mut self) -> bool {
        self.fetcher.allow_remote();
        self.refresh()
    }

    pub(crate) fn retry(&mut self, index: usize) -> bool {
        let Some(source) = self.sources.get(index) else {
            return false;
        };
        if !matches!(self.loaded[index], Loaded::Failed(error) if error.retryable()) {
            return false;
        }
        let key = cache_key(&self.fetcher, source, &format!("image-{index}"));
        self.fetcher.resources().forget::<ImageLoader>(&key);
        self.loaded[index] = Loaded::Loading;
        self.refresh();
        true
    }

    pub(crate) fn view(
        &self,
        block: &ImageBlock,
        theme: ArtisanTheme,
        id: &str,
        width: f32,
        emit: Rc<dyn Fn(Action)>,
    ) -> Result<aqua_ui::element::Element, &'static str> {
        if !width.is_finite() || width <= 0.0 {
            return Err("Image gallery needs its measured content width.");
        }
        let gallery = block.images.len() > 1;
        let mut root = UiBox::new()
            .layout("column")
            .width(Length::Pct(100.0))
            .spaced(Space::S8);
        let mut index = 0;
        for columns in gallery_rows(block.images.len()) {
            let count = usize::from(columns);
            let tile_width =
                (width - Space::S8.px() * (f32::from(columns) - 1.0)).max(0.0) / f32::from(columns);
            let mut row = UiBox::new().width(width).spaced(Space::S8);
            for _ in 0..count {
                let picture = &block.images[index];
                let loaded = &self.loaded[index];
                let aspect = if gallery {
                    1.0
                } else {
                    image_aspect(loaded, block.hint).max(16.0 / 9.0)
                };
                let shown_width = if gallery {
                    tile_width
                } else {
                    tile_width.min(480.0 * aspect)
                };
                let height = shown_width / aspect;
                let alt = picture
                    .alt
                    .as_deref()
                    .filter(|alt| !alt.trim().is_empty())
                    .unwrap_or_else(|| picture.src.raw());
                let path = picture.src.raw().to_owned();
                let slot = index;
                let retry =
                    matches!(loaded, Loaded::Failed(error) if error.retryable()).then(|| {
                        let retry = Rc::clone(&emit);
                        Button::text(
                            format!("{id}-retry-{index}"),
                            theme,
                            ButtonTone::Neutral,
                            "Try again",
                        )
                        .size(ButtonSize::Sm)
                        .on_activate(move |_| retry(Action::Retry(slot)))
                        .build()
                    });
                let mut frame = Surface::Media(Framing::Frame)
                    .build(
                        &theme,
                        vec![content(loaded, &theme, &path, shown_width, height, retry)],
                    )
                    .style(
                        Style::new()
                            .width(Length::Px(shown_width))
                            .height(Length::Px(height)),
                    )
                    .with_key(format!("{id}-image-{index}"))
                    .role(aqua_core::Role::Image)
                    .aria_label(alt.to_owned());
                let activation = match loaded {
                    Loaded::Ready(image) => Some(Action::Open {
                        index: slot,
                        image: image.clone(),
                    }),
                    Loaded::Failed(ImageError::NeedsConsent) => Some(Action::AllowRemote),
                    Loaded::Loading | Loaded::Failed(_) => None,
                };
                let presses = matches!(loaded, Loaded::Ready(_));
                let pressed = Rc::new(Cell::new(None));
                let down = Rc::clone(&pressed);
                let anchor = Rc::clone(&self.anchors[index]);
                let key_anchor = Rc::clone(&anchor);
                let entered = Rc::clone(&anchor);
                let keyed = Rc::clone(&emit);
                let requested = Rc::clone(&emit);
                let copied = Rc::clone(&emit);
                frame =
                    frame
                        .on_input(InputKind::PointerEnter, EventPhase::Bubble, move |event| {
                            entered.set(Some(menu_anchor(event.bounds)));
                            InputResponse::default()
                        })
                        .on_input(InputKind::KeyDown, EventPhase::Bubble, move |event| {
                            let position = menu_anchor(event.bounds);
                            key_anchor.set(Some(position));
                            let RoutedInput::Native(InputEvent::KeyDown(key)) = &event.input else {
                                return InputResponse::default();
                            };
                            let action = if opens_context_menu(key) {
                                Some(Action::CopyPath {
                                    index: slot,
                                    position,
                                })
                            } else if matches!(
                                key.key,
                                aqua_core::Key::Char(' ')
                                    | aqua_core::Key::Named(aqua_core::NamedKey::Enter)
                            ) {
                                activation.clone()
                            } else {
                                None
                            };
                            let Some(action) = action else {
                                return InputResponse::default();
                            };
                            keyed(action);
                            InputResponse {
                                stop_propagation: true,
                                ..InputResponse::handled()
                            }
                        })
                        .on_accessibility_request(move |request| {
                            if request == AccessibilityRequest::ShowContextMenu {
                                requested(Action::CopyPath {
                                    index: slot,
                                    position: anchor.get().unwrap_or(aqua_core::Point::ZERO),
                                });
                            }
                        })
                        .on_input(InputKind::PointerDown, EventPhase::Bubble, move |event| {
                            match &event.input {
                                RoutedInput::Native(InputEvent::PointerDown(
                                    pointer,
                                    PointerButton::Secondary,
                                )) => {
                                    copied(Action::CopyPath {
                                        index: slot,
                                        position: pointer.position,
                                    });
                                    InputResponse {
                                        stop_propagation: true,
                                        ..InputResponse::handled()
                                    }
                                }
                                RoutedInput::Native(InputEvent::PointerDown(
                                    pointer,
                                    PointerButton::Primary,
                                )) if presses => {
                                    down.set(Some(pointer.contact));
                                    InputResponse {
                                        focus: aqua_ui::input::FocusRequest::Focus,
                                        ..InputResponse::capture()
                                    }
                                }
                                _ => InputResponse::default(),
                            }
                        });
                match loaded {
                    Loaded::Ready(image) => {
                        let image = image.clone();
                        let cancelled = Rc::clone(&pressed);
                        let activate = Rc::clone(&emit);
                        let access = Rc::clone(&emit);
                        let accessible_image = image.clone();
                        frame = frame
                            .role(aqua_core::Role::Button)
                            .aria_label(format!("{alt}. Open image preview"))
                            .focusable(true)
                            .cursor(aqua_core::CursorIcon::Pointer)
                            .on_accessibility_activate(move || {
                                access(Action::Open {
                                    index: slot,
                                    image: accessible_image.clone(),
                                });
                            })
                            .on_input(InputKind::PointerCancel, EventPhase::Bubble, move |_| {
                                cancelled.set(None);
                                InputResponse {
                                    pointer_capture: aqua_ui::input::CaptureRequest::Release,
                                    ..InputResponse::default()
                                }
                            })
                            .on_input(InputKind::PointerUp, EventPhase::Bubble, move |event| {
                                let pressed = pressed.take();
                                if let RoutedInput::Native(InputEvent::PointerUp(
                                    pointer,
                                    PointerButton::Primary,
                                )) = &event.input
                                    && pressed == Some(pointer.contact)
                                    && event.bounds.contains(pointer.position)
                                {
                                    activate(Action::Open {
                                        index: slot,
                                        image: image.clone(),
                                    });
                                }
                                InputResponse {
                                    pointer_capture: aqua_ui::input::CaptureRequest::Release,
                                    ..InputResponse::default()
                                }
                            });
                    }
                    Loaded::Failed(ImageError::NeedsConsent) => {
                        let click = Rc::clone(&emit);
                        let access = Rc::clone(&emit);
                        frame = frame
                            .role(aqua_core::Role::Button)
                            .aria_label(format!("{alt}. Load remote images"))
                            .focusable(true)
                            .cursor(aqua_core::CursorIcon::Pointer)
                            .on_accessibility_activate(move || access(Action::AllowRemote))
                            .on_input(InputKind::PointerUp, EventPhase::Bubble, move |event| {
                                if matches!(
                                    &event.input,
                                    RoutedInput::Native(InputEvent::PointerUp(
                                        _,
                                        PointerButton::Primary
                                    ))
                                ) {
                                    click(Action::AllowRemote);
                                    InputResponse::handled()
                                } else {
                                    InputResponse::default()
                                }
                            });
                    }
                    Loaded::Failed(
                        ImageError::Load(MediaLoadError::Fetch(_)) | ImageError::Decode,
                    ) => {
                        frame = frame.role(aqua_core::Role::Group);
                    }
                    Loaded::Loading
                    | Loaded::Failed(ImageError::Load(
                        MediaLoadError::Invalid | MediaLoadError::TooLarge,
                    )) => {}
                }
                let mut column = UiBox::new()
                    .layout("column")
                    .width(shown_width)
                    .spaced(Space::S8)
                    .child(frame);
                if let Some(caption) = picture
                    .caption
                    .as_deref()
                    .map(str::trim)
                    .filter(|caption| !caption.is_empty())
                {
                    column = column.child(
                        TextRole::Caption
                            .text(&theme, caption.to_owned())
                            .gpui_source_color(theme.colors.text_muted.to_reference_text_color())
                            .build(),
                    );
                }
                row = row.child(column.build());
                index += 1;
            }
            root = root.child(row.build());
        }
        Ok(root.build())
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "image dimensions are bounded by the decoder"
)]
fn image_aspect(image: &Loaded, hint: super::super::SizeHint) -> f32 {
    if let Loaded::Ready(image) = image
        && let ImageStatus::Ready { width, height } = image.status()
        && width > 0
        && height > 0
    {
        return (width as f32 / height as f32).clamp(0.05, 20.0);
    }
    hint.aspect().unwrap_or(16.0 / 9.0)
}

fn content(
    image: &Loaded,
    theme: &ArtisanTheme,
    source: &str,
    width: f32,
    height: f32,
    retry: Option<aqua_ui::element::Element>,
) -> aqua_ui::element::Element {
    match image {
        Loaded::Ready(image) => UiBox::new()
            .width(width)
            .height(height)
            .clip(true)
            .child(
                UiBox::new()
                    .width(width + 96.0)
                    .height(height + 96.0)
                    .style(
                        Style::new()
                            .absolute_at(Some(-48.0), Some(-48.0), None, None)
                            .content_blur(24.0),
                    )
                    .child(
                        Media::new(image.clone())
                            .width(width + 96.0)
                            .height(height + 96.0)
                            .fit(aqua_core::image::ImageFit::Cover)
                            .build(),
                    )
                    .build(),
            )
            .child(
                Media::new(image.clone())
                    .width(width)
                    .height(height)
                    .fit(aqua_core::image::ImageFit::Contain)
                    .style(Style::new().absolute_at(Some(0.0), Some(0.0), None, None))
                    .build(),
            )
            .build(),
        Loaded::Loading | Loaded::Failed(_) => {
            let mut panel = UiBox::new()
                .layout("column")
                .width(width)
                .height(height)
                .spaced(Space::S4)
                .padded(Space::S12)
                .style(
                    Style::new()
                        .align(aqua_ui::AlignItems::CENTER)
                        .justify(aqua_ui::JustifyContent::CENTER)
                        .gpui_background(aqua_core::GpuiBackground::Solid(
                            theme.colors.surface.to_reference_paint(),
                        )),
                );
            let text = |value: String, color: crate::ui::theme::Oklch| {
                Text::new(value)
                    .text_style(TextRole::Label.style(theme))
                    .gpui_source_color(color.to_reference_text_color())
                    .overflow(aqua_text::overflow::OverflowMode::EndEllipsis)
                    .build()
            };
            if let Loaded::Failed(error) = image {
                let title = error.summary();
                let (icon, hint) = match error {
                    ImageError::NeedsConsent => (
                        editor_assets::AssetId::TABLER_WORLD,
                        Some("Load remote images"),
                    ),
                    ImageError::Load(
                        MediaLoadError::Invalid
                        | MediaLoadError::TooLarge
                        | MediaLoadError::Fetch(_),
                    )
                    | ImageError::Decode => (editor_assets::AssetId::TABLER_FILE_OFF, None),
                };
                panel = panel
                    .child(IconSize::Lg.glyph(icon, theme.colors.text_muted.to_reference_paint()))
                    .child(text(title.to_owned(), theme.colors.text))
                    .child(text(source.trim().to_owned(), theme.colors.text_muted));
                if let Some(hint) = hint {
                    panel = panel.child(text(hint.to_owned(), theme.colors.text_muted));
                }
                panel = panel.children(retry);
            } else {
                panel = panel.child(text("Loading image…".to_owned(), theme.colors.text_muted));
            }
            panel.build()
        }
    }
}

#[cfg(test)]
#[path = "../../../../../tests/ui/rich/media/image.rs"]
mod tests;
