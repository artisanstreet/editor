use std::{
    cell::Cell,
    num::NonZeroUsize,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use aqua::images::{ImageHandle, ImageStatus};
use aqua_core::{InputEvent, PointerButton};
use aqua_ui::{
    element::{Box as UiBox, Media, Text},
    input::{EventPhase, InputKind, InputResponse, RoutedInput},
    style::{Length, Style},
};

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
    waiters: Vec<u64>,
    wake: aqua::mailbox::Mailbox<()>,
}

impl ImageState {
    pub(crate) fn new(block: &ImageBlock, fetcher: MediaFetcher, loader: Images) -> Self {
        let sources: Vec<_> = block.images.iter().map(|image| image.src.clone()).collect();
        let mut state = Self {
            loaded: sources.iter().map(|_| Loaded::Loading).collect(),
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
            let sender = self.wake.sender();
            let wake: aqua::mailbox::Wake = Arc::new(move || {
                let _ = sender.send(());
            });
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

    pub(crate) fn retry(&mut self, index: usize) -> bool {
        let Some(source) = self.sources.get(index) else {
            return false;
        };
        if !matches!(self.loaded[index], Loaded::Failed(error) if error != ImageError::Load(MediaLoadError::Invalid))
        {
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
            .gap(8.0);
        let mut index = 0;
        for columns in gallery_rows(block.images.len()) {
            let count = usize::from(columns);
            let tile_width =
                (width - 8.0 * (f32::from(columns) - 1.0)).max(0.0) / f32::from(columns);
            let mut row = UiBox::new().width(width).gap(8.0);
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
                    .unwrap_or(&picture.src.raw);
                let path = picture.src.raw.clone();
                let mut frame = UiBox::new()
                    .width(shown_width)
                    .height(height)
                    .radius(crate::ui::theme::RadiusTokens::value(
                        crate::ui::theme::RadiusStep::Xl,
                    ))
                    .clip(true)
                    .style(
                        Style::new().gpui_shadows(
                            crate::ui::glass::card_shadows(&theme)
                                .into_iter()
                                .map(Into::into)
                                .collect(),
                        ),
                    )
                    .child(content(loaded, &theme, &path, shown_width, height))
                    .build()
                    .with_key(format!("{id}-image-{index}"))
                    .role(aqua_core::Role::Image)
                    .aria_label(alt.to_owned());
                let copied = Rc::clone(&emit);
                let slot = index;
                frame = frame.on_input(InputKind::PointerDown, EventPhase::Bubble, move |event| {
                    if let RoutedInput::Native(InputEvent::PointerDown(
                        pointer,
                        PointerButton::Secondary,
                    )) = &event.input
                    {
                        copied(Action::CopyPath {
                            index: slot,
                            position: pointer.position,
                        });
                        InputResponse {
                            stop_propagation: true,
                            ..InputResponse::handled()
                        }
                    } else {
                        InputResponse::default()
                    }
                });
                match loaded {
                    Loaded::Ready(image) => {
                        let image = image.clone();
                        let pressed = Rc::new(Cell::new(None));
                        let down = Rc::clone(&pressed);
                        let cancelled = Rc::clone(&pressed);
                        let activate = Rc::clone(&emit);
                        let access = Rc::clone(&emit);
                        let accessible_image = image.clone();
                        let key_image = image.clone();
                        let key_emit = Rc::clone(&emit);
                        frame = frame
                            .role(aqua_core::Role::Button)
                            .aria_label(format!("{alt}. Open image preview"))
                            .focusable(true)
                            .cursor(aqua_core::CursorIcon::Pointer)
                            .on_accessibility_activate(move || {
                                access(Action::Open {
                                    index: slot,
                                    image: accessible_image.clone(),
                                })
                            })
                            .on_input(InputKind::PointerDown, EventPhase::Bubble, move |event| {
                                if let RoutedInput::Native(InputEvent::PointerDown(pointer, PointerButton::Primary)) = &event.input {
                                    down.set(Some(pointer.contact));
                                    InputResponse { focus: aqua_ui::input::FocusRequest::Focus, ..InputResponse::capture() }
                                } else { InputResponse::default() }
                            })
                            .on_input(InputKind::PointerCancel, EventPhase::Bubble, move |_| {
                                cancelled.set(None);
                                InputResponse { pointer_capture: aqua_ui::input::CaptureRequest::Release, ..InputResponse::default() }
                            })
                            .on_input(InputKind::KeyDown, EventPhase::Bubble, move |event| {
                                if matches!(&event.input, RoutedInput::Native(InputEvent::KeyDown(key)) if matches!(key.key, aqua_core::Key::Char(' ') | aqua_core::Key::Named(aqua_core::NamedKey::Enter))) {
                                    key_emit(Action::Open { index: slot, image: key_image.clone() });
                                    InputResponse { stop_propagation: true, ..InputResponse::handled() }
                                } else { InputResponse::default() }
                            })
                            .on_input(InputKind::PointerUp, EventPhase::Bubble, move |event| {
                                let pressed = pressed.take();
                                if let RoutedInput::Native(InputEvent::PointerUp(pointer, PointerButton::Primary)) = &event.input {
                                    if pressed == Some(pointer.contact) && event.bounds.contains(pointer.position) {
                                        activate(Action::Open { index: slot, image: image.clone() });
                                    }
                                }
                                InputResponse { pointer_capture: aqua_ui::input::CaptureRequest::Release, ..InputResponse::default() }
                            });
                    }
                    Loaded::Failed(error)
                        if *error != ImageError::Load(MediaLoadError::Invalid) =>
                    {
                        let retry = Rc::clone(&emit);
                        frame = frame.cursor(aqua_core::CursorIcon::Pointer).on_input(
                            InputKind::PointerUp,
                            EventPhase::Bubble,
                            move |event| {
                                if matches!(
                                    &event.input,
                                    RoutedInput::Native(InputEvent::PointerUp(
                                        _,
                                        PointerButton::Primary
                                    ))
                                ) {
                                    retry(Action::Retry(slot));
                                    InputResponse::handled()
                                } else {
                                    InputResponse::default()
                                }
                            },
                        );
                    }
                    _ => {}
                }
                let mut column = UiBox::new()
                    .layout("column")
                    .width(shown_width)
                    .gap(6.0)
                    .child(frame);
                if let Some(caption) = picture
                    .caption
                    .as_deref()
                    .map(str::trim)
                    .filter(|caption| !caption.is_empty())
                {
                    column = column.child(
                        Text::new(caption.to_owned())
                            .font_size(13.0)
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
    if let Loaded::Ready(image) = image {
        if let ImageStatus::Ready { width, height } = image.status() {
            if width > 0 && height > 0 {
                return (width as f32 / height as f32).clamp(0.05, 20.0);
            }
        }
    }
    hint.aspect().unwrap_or(16.0 / 9.0)
}

fn content(
    image: &Loaded,
    theme: &ArtisanTheme,
    source: &str,
    width: f32,
    height: f32,
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
                .gap(if matches!(image, Loaded::Loading) {
                    6.0
                } else {
                    4.0
                })
                .style(
                    Style::new()
                        .align(aqua_ui::AlignItems::CENTER)
                        .justify(aqua_ui::JustifyContent::CENTER)
                        .padding(Length::Px(12.0))
                        .gpui_background(aqua_core::GpuiBackground::Solid(
                            theme.colors.surface.to_reference_paint(),
                        )),
                );
            let text = |value: String, color: crate::ui::theme::Oklch| {
                Text::new(value)
                    .font_size(13.0)
                    .gpui_source_color(color.to_reference_text_color())
                    .overflow(aqua_text::overflow::OverflowMode::EndEllipsis)
                    .build()
            };
            if let Loaded::Failed(error) = image {
                let title = match error {
                    ImageError::Load(MediaLoadError::Invalid) => "Not a valid image path",
                    ImageError::Load(MediaLoadError::Fetch(
                        crate::ui::media::source::MediaFailure::Missing,
                    )) => "File not found",
                    ImageError::Load(MediaLoadError::Fetch(
                        crate::ui::media::source::MediaFailure::Unreachable,
                    )) => "The host didn't answer",
                    ImageError::Load(MediaLoadError::Fetch(
                        crate::ui::media::source::MediaFailure::Unreadable,
                    )) => "Couldn't read the file",
                    ImageError::Decode => "Not a supported image",
                };
                panel = panel
                    .child(crate::ui::asset::source_asset_glyph(
                        editor_assets::AssetId::TABLER_FILE_OFF,
                        20.0,
                        theme.colors.text_muted.to_reference_paint(),
                    ))
                    .child(text(title.to_owned(), theme.colors.text))
                    .child(text(source.trim().to_owned(), theme.colors.text_muted));
                if *error != ImageError::Load(MediaLoadError::Invalid) {
                    panel = panel.child(
                        UiBox::new()
                            .style(Style::new().padding_insets(aqua_core::Insets {
                                top: 4.0,
                                ..Default::default()
                            }))
                            .child(text("Click to retry".to_owned(), theme.colors.text_muted))
                            .build(),
                    );
                }
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
