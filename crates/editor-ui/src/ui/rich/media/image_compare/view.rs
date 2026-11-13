use std::{
    num::NonZeroUsize,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use aqua::images::{ImageHandle, ImageStatus};
use aqua_core::{InputEvent, Key, NamedKey, PointerButton};
use aqua_ui::input::{CaptureRequest, FocusRequest, InputKind, InputResponse, RoutedInput};

use super::{ImageCompare, Mode};
use crate::ui::kit::{IconSize, Media, Space, Spaced, Surface, TextRole};
use crate::ui::media::{
    image::{ImageError, Images},
    source::{MediaFailure, MediaFetcher, MediaLoadError, MediaSource},
};

static NEXT_WAITER: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Side {
    Before,
    After,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Failure {
    Invalid,
    Fetch(MediaFailure),
    Undecodable,
}

impl Failure {
    fn title(self) -> &'static str {
        match self {
            Self::Invalid => "Not a valid image path",
            Self::Fetch(MediaFailure::Missing) => "File not found",
            Self::Fetch(MediaFailure::Unreachable) => "The host didn't answer",
            Self::Fetch(MediaFailure::Unreadable) => "Couldn't read the file",
            Self::Undecodable => "Not a supported image",
        }
    }
}

enum ImageState {
    Loading,
    Decoding(ImageHandle),
    Failed(Failure),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Action {
    Mode(usize),
    Press { x: f32, width: f32, clicks: usize },
    Move { x: f32, width: f32 },
    Release,
    Key { key: Key, shift: bool },
}

pub(crate) struct CompareState {
    mode: Mode,
    split: f32,
    showing_after: bool,
    dragging: bool,
    before: MediaSource,
    after: MediaSource,
    fetcher: MediaFetcher,
    loader: Images,
    images: [ImageState; 2],
    observed: [Option<ImageStatus>; 2],
    waiters: [u64; 2],
    wake: aqua::mailbox::Mailbox<()>,
}

impl CompareState {
    pub(crate) fn new(model: &ImageCompare, fetcher: MediaFetcher, loader: Images) -> Self {
        let mut state = Self {
            mode: model.mode,
            split: 0.5,
            showing_after: false,
            dragging: false,
            before: model.before.src.clone(),
            after: model.after.src.clone(),
            fetcher,
            loader,
            images: [ImageState::Loading, ImageState::Loading],
            observed: [None, None],
            waiters: [
                NEXT_WAITER.fetch_add(1, Ordering::Relaxed),
                NEXT_WAITER.fetch_add(1, Ordering::Relaxed),
            ],
            wake: aqua::mailbox::Mailbox::new(NonZeroUsize::MIN),
        };
        state.refresh();
        state
    }

    pub(crate) fn wake_mailbox(&self) -> &aqua::mailbox::Mailbox<()> {
        &self.wake
    }

    fn index(side: Side) -> usize {
        match side {
            Side::Before => 0,
            Side::After => 1,
        }
    }

    pub(crate) fn refresh(&mut self) -> bool {
        self.wake.drain();
        let mut changed = false;
        for side in [Side::Before, Side::After] {
            let index = Self::index(side);
            let source = match side {
                Side::Before => &self.before,
                Side::After => &self.after,
            };
            let sender = self.wake.sender();
            let wake: aqua::mailbox::Wake = Arc::new(move || {
                let _ = sender.send(());
            });
            let part = match side {
                Side::Before => "before",
                Side::After => "after",
            };
            let result =
                self.loader
                    .request(&self.fetcher, source, part, self.waiters[index], wake);
            let next = match result {
                None => ImageState::Loading,
                Some(Ok(image)) => ImageState::Decoding(image),
                Some(Err(ImageError::Load(MediaLoadError::Invalid))) => {
                    ImageState::Failed(Failure::Invalid)
                }
                Some(Err(ImageError::Load(MediaLoadError::Fetch(error)))) => {
                    ImageState::Failed(Failure::Fetch(error))
                }
                Some(Err(ImageError::Decode)) => ImageState::Failed(Failure::Undecodable),
            };
            let same = match (&self.images[index], &next) {
                (ImageState::Loading, ImageState::Loading) => true,
                (ImageState::Failed(a), ImageState::Failed(b)) => a == b,
                (ImageState::Decoding(a), ImageState::Decoding(b)) => a.id() == b.id(),
                _ => false,
            };
            let status = match &next {
                ImageState::Decoding(image) => Some(image.status()),
                _ => None,
            };
            if !same || self.observed[index] != status {
                changed = true;
            }
            self.observed[index] = status;
            self.images[index] = next;
        }
        changed
    }

    pub(crate) fn reconcile(
        &mut self,
        model: &ImageCompare,
        fetcher: MediaFetcher,
        scope_changed: bool,
    ) -> bool {
        if !scope_changed
            && self.before.raw == model.before.src.raw
            && self.after.raw == model.after.src.raw
        {
            return false;
        }
        self.before = model.before.src.clone();
        self.after = model.after.src.clone();
        self.fetcher = fetcher;
        self.images = [ImageState::Loading, ImageState::Loading];
        self.observed = [None, None];
        self.refresh();
        true
    }

    fn set_split(&mut self, split: f32) -> bool {
        let split = split.clamp(0.0, 1.0);
        let moved = (split - self.split).abs() > f32::EPSILON;
        self.split = split;
        moved
    }

    fn follow(&mut self, x: f32, width: f32) -> bool {
        width.is_finite() && width > 0.0 && x.is_finite() && self.set_split(x / width)
    }

    pub(crate) fn dispatch(&mut self, action: Action) -> bool {
        match action {
            Action::Mode(index) => {
                let Some(mode) = Mode::ALL.get(index).copied() else {
                    return false;
                };
                if self.mode == mode {
                    return false;
                }
                self.mode = mode;
                self.dragging = false;
                true
            }
            Action::Press { x, width, clicks } => match self.mode {
                Mode::SideBySide => false,
                Mode::Toggle => {
                    self.showing_after = !self.showing_after;
                    true
                }
                Mode::Slider | Mode::Blend if clicks >= 2 => self.set_split(0.5),
                Mode::Slider | Mode::Blend => {
                    self.dragging = true;
                    self.follow(x, width);
                    true
                }
            },
            Action::Move { x, width } => self.dragging && self.follow(x, width),
            Action::Release => std::mem::take(&mut self.dragging),
            Action::Key { key, shift } => {
                let step = if shift { 0.01 } else { 0.05 };
                match (self.mode, key) {
                    (
                        Mode::Toggle,
                        Key::Char(' ')
                        | Key::Named(NamedKey::Enter | NamedKey::Left | NamedKey::Right),
                    ) => {
                        self.showing_after = !self.showing_after;
                        true
                    }
                    (Mode::Slider | Mode::Blend, Key::Named(NamedKey::Left)) => {
                        self.set_split(self.split - step)
                    }
                    (Mode::Slider | Mode::Blend, Key::Named(NamedKey::Right)) => {
                        self.set_split(self.split + step)
                    }
                    (Mode::Slider | Mode::Blend, Key::Named(NamedKey::Home)) => self.set_split(0.0),
                    (Mode::Slider | Mode::Blend, Key::Named(NamedKey::End)) => self.set_split(1.0),
                    _ => false,
                }
            }
        }
    }
}

impl CompareState {
    pub(crate) fn view(
        &self,
        model: &ImageCompare,
        theme: crate::ui::theme::ArtisanTheme,
        id: &str,
        available_width: f32,
        emit: Rc<dyn Fn(Action)>,
    ) -> Result<aqua_ui::element::Element, &'static str> {
        use crate::ui::segmented_control::{Segment, SegmentedControl};
        use aqua_ui::{
            AlignItems,
            element::Box as UiBox,
            style::{Length, Style},
        };
        if !available_width.is_finite() || available_width <= 0.0 {
            return Err("Image comparison needs its measured content width.");
        }
        let (aspect, max_width) = frame_size(&self.images, model.hint);
        let before_label = model.before.label_or("Before");
        let after_label = model.after.label_or("After");
        let view = if self.mode == Mode::SideBySide {
            let width = available_width.min(max_width * 2.0 + 8.0);
            let half = ((width - 8.0) / 2.0).max(0.0);
            UiBox::new()
                .width(width)
                .spaced(Space::S8)
                .child(frame(
                    &theme,
                    half,
                    aspect,
                    image_layer(
                        &self.images[0],
                        &theme,
                        &self.before.raw,
                        half,
                        half / aspect,
                    ),
                    vec![corner(&theme, before_label.clone(), true, false, half)],
                    None,
                ))
                .child(frame(
                    &theme,
                    half,
                    aspect,
                    image_layer(
                        &self.images[1],
                        &theme,
                        &self.after.raw,
                        half,
                        half / aspect,
                    ),
                    vec![corner(&theme, after_label.clone(), true, false, half)],
                    None,
                ))
                .build()
        } else {
            let width = available_width.min(max_width);
            let height = width / aspect;
            let mut layers = Vec::new();
            match self.mode {
                Mode::Slider => {
                    layers.push(image_layer(
                        &self.images[1],
                        &theme,
                        &self.after.raw,
                        width,
                        height,
                    ));
                    layers.push(
                        UiBox::new()
                            .width(width * self.split)
                            .height(height)
                            .clip(true)
                            .style(Style::new().absolute_at(Some(0.0), Some(0.0), None, None))
                            .child(image_layer(
                                &self.images[0],
                                &theme,
                                &self.before.raw,
                                width,
                                height,
                            ))
                            .build(),
                    );
                    layers.push(corner(&theme, before_label, true, self.dragging, width));
                    layers.push(corner(&theme, after_label, false, self.dragging, width));
                    layers.extend(divider(&theme, self.split, width, height));
                }
                Mode::Toggle => {
                    let (image, source, label, left) = if self.showing_after {
                        (&self.images[1], &self.after.raw, after_label, false)
                    } else {
                        (&self.images[0], &self.before.raw, before_label, true)
                    };
                    layers.push(image_layer(image, &theme, source, width, height));
                    layers.push(corner(&theme, label, left, false, width));
                }
                Mode::Blend => {
                    layers.push(image_layer(
                        &self.images[0],
                        &theme,
                        &self.before.raw,
                        width,
                        height,
                    ));
                    layers.push(
                        UiBox::new()
                            .width(width)
                            .height(height)
                            .opacity(self.split)
                            .style(Style::new().absolute_at(Some(0.0), Some(0.0), None, None))
                            .child(image_layer(
                                &self.images[1],
                                &theme,
                                &self.after.raw,
                                width,
                                height,
                            ))
                            .build(),
                    );
                    let text = format!(
                        "{} {:.0}% · {} {:.0}%",
                        before_label,
                        (1.0 - self.split) * 100.0,
                        after_label,
                        self.split * 100.0
                    );
                    layers.push(
                        UiBox::new()
                            .width(width)
                            .style(
                                Style::new()
                                    .justify(aqua_ui::JustifyContent::CENTER)
                                    .absolute_at(Some(0.0), None, None, Some(8.0)),
                            )
                            .child(pill(&theme, text))
                            .build(),
                    );
                }
                Mode::SideBySide => unreachable!(),
            }
            let frame = frame(
                &theme,
                width,
                aspect,
                UiBox::new().build(),
                layers,
                Some((self.mode, self.split, emit.clone())),
            );
            frame.with_key(format!("{id}-frame"))
        };
        let mode_index = Mode::ALL
            .iter()
            .position(|mode| *mode == self.mode)
            .unwrap_or(0);
        let switch_emit = emit;
        let control = SegmentedControl::new(
            format!("{id}-modes"),
            theme,
            "Comparison mode",
            mode_index,
            Segment::new(0, Mode::ALL[0].label()),
        )
        .options(
            Mode::ALL[1..]
                .iter()
                .enumerate()
                .map(|(index, mode)| Segment::new(index + 1, mode.label())),
        )
        .on_change(move |index| switch_emit(Action::Mode(index)))
        .build();
        Ok(UiBox::new()
            .layout("column")
            .width(Length::Pct(100.0))
            .spaced(Space::S8)
            .style(Style::new().align(AlignItems::CENTER))
            .child(view)
            .child(control)
            .build())
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "image dimensions are bounded by the decoder"
)]
fn frame_size(images: &[ImageState; 2], hint: super::super::SizeHint) -> (f32, f32) {
    let widest = images
        .iter()
        .filter_map(|image| match image {
            ImageState::Decoding(image) => match image.status() {
                ImageStatus::Ready { width, height } if width > 0 && height > 0 => Some((
                    (width as f32 / height as f32).clamp(0.05, 20.0),
                    width as f32,
                )),
                _ => None,
            },
            _ => None,
        })
        .max_by(|a, b| a.0.total_cmp(&b.0));
    let (aspect, natural) = widest.map_or_else(
        || (hint.aspect().unwrap_or(16.0 / 9.0), hint.width),
        |(aspect, width)| (aspect, Some(width)),
    );
    let cap = 480.0 * aspect;
    (
        aspect,
        natural.map_or(cap, |width| width.min(cap)).max(48.0),
    )
}

fn image_layer(
    image: &ImageState,
    theme: &crate::ui::theme::ArtisanTheme,
    source: &str,
    width: f32,
    height: f32,
) -> aqua_ui::element::Element {
    use aqua_ui::{
        AlignItems, JustifyContent,
        element::{Box as UiBox, Media, Text},
        style::{Length, Style},
    };
    let status = match image {
        ImageState::Loading => None,
        ImageState::Failed(error) => Some(*error),
        ImageState::Decoding(image) => match image.status() {
            ImageStatus::Ready { .. } => {
                return Media::new(image.clone())
                    .width(width)
                    .height(height)
                    .fit(aqua_core::image::ImageFit::Contain)
                    .style(Style::new().absolute_at(Some(0.0), Some(0.0), None, None))
                    .build();
            }
            ImageStatus::Pending => None,
            ImageStatus::Failed(_) | ImageStatus::Cancelled => Some(Failure::Undecodable),
        },
    };
    let mut panel = UiBox::new()
        .layout("column")
        .width(width)
        .height(height)
        .spaced(Space::S4)
        .padded(Space::S12)
        .style(
            Style::new()
                .absolute_at(Some(0.0), Some(0.0), None, None)
                .align(AlignItems::CENTER)
                .justify(JustifyContent::CENTER)
                .gpui_background(aqua_core::GpuiBackground::Solid(
                    theme.colors.surface.to_reference_paint(),
                )),
        );
    let text = |value: String, color: crate::ui::theme::Oklch| {
        Text::new(value)
            .text_style(TextRole::Label.style(theme))
            .gpui_source_color(color.to_reference_text_color())
            .overflow(aqua_text::overflow::OverflowMode::EndEllipsis)
            .style(
                Style::new()
                    .min_width(Length::Px(0.0))
                    .max_width(Length::Pct(100.0)),
            )
            .build()
    };
    if let Some(error) = status {
        panel = panel
            .child(IconSize::Lg.glyph(
                editor_assets::AssetId::TABLER_FILE_OFF,
                theme.colors.text_muted.to_reference_paint(),
            ))
            .child(text(error.title().to_owned(), theme.colors.text))
            .child(text(source.trim().to_owned(), theme.colors.text_muted));
        if error != Failure::Invalid {
            panel = panel.child(text("Click to retry".to_owned(), theme.colors.text_muted));
        }
    } else {
        panel = panel.child(text("Loading image…".to_owned(), theme.colors.text_muted));
    }
    panel.build()
}

fn pill(theme: &crate::ui::theme::ArtisanTheme, text: String) -> aqua_ui::element::Element {
    let height = TextRole::Caption.spec().line + 2.0 * Surface::Pill(0.0).padding_px();
    Surface::Pill(height).build(
        theme,
        vec![TextRole::Caption.text(theme, text).wrap(false).build()],
    )
}

fn corner(
    theme: &crate::ui::theme::ArtisanTheme,
    text: String,
    left: bool,
    hidden: bool,
    _width: f32,
) -> aqua_ui::element::Element {
    use aqua_ui::{element::Box as UiBox, style::Style};
    UiBox::new()
        .opacity(if hidden { 0.0 } else { 1.0 })
        .style(Style::new().absolute_at(
            if left { Some(Space::S8.px()) } else { None },
            Some(Space::S8.px()),
            if left { None } else { Some(Space::S8.px()) },
            None,
        ))
        .child(pill(theme, text))
        .build()
}

fn divider(
    theme: &crate::ui::theme::ArtisanTheme,
    split: f32,
    width: f32,
    height: f32,
) -> Vec<aqua_ui::element::Element> {
    use crate::ui::theme::Opacity;
    use aqua_ui::{AlignItems, JustifyContent, element::Box as UiBox, style::Style};
    let x = split * width;
    let line = UiBox::new()
        .width(1.0)
        .height(height)
        .style(
            Style::new()
                .absolute_at(Some(x - 0.5), Some(0.0), None, None)
                .gpui_background(aqua_core::GpuiBackground::Solid(
                    theme
                        .colors
                        .background
                        .at(Opacity::Backdrop)
                        .to_reference_paint(),
                )),
        )
        .build();
    let edge = 2.0 * (IconSize::Sm.px() + Surface::Pill(0.0).padding_px());
    let handle = Surface::Pill(edge)
        .build(
            theme,
            vec![
                UiBox::new()
                    .layout("row")
                    .style(
                        Style::new()
                            .align(AlignItems::CENTER)
                            .justify(JustifyContent::CENTER),
                    )
                    .child(IconSize::Sm.glyph(
                        editor_assets::AssetId::TABLER_CHEVRON_LEFT,
                        theme.colors.text.to_reference_paint(),
                    ))
                    .child(IconSize::Sm.glyph(
                        editor_assets::AssetId::TABLER_CHEVRON_RIGHT,
                        theme.colors.text.to_reference_paint(),
                    ))
                    .build(),
            ],
        )
        .style(
            Style::new()
                .width(aqua_ui::style::Length::Px(edge))
                .absolute_at(
                    Some(x - edge / 2.0),
                    Some(height / 2.0 - edge / 2.0),
                    None,
                    None,
                ),
        );
    vec![line, handle]
}

fn frame(
    theme: &crate::ui::theme::ArtisanTheme,
    width: f32,
    aspect: f32,
    base: aqua_ui::element::Element,
    layers: Vec<aqua_ui::element::Element>,
    interactive: Option<(Mode, f32, Rc<dyn Fn(Action)>)>,
) -> aqua_ui::element::Element {
    use aqua_ui::{input::EventPhase, style::Style};
    let mut children = vec![base];
    children.extend(layers);
    let root = Surface::Media(Media::Frame).build(theme, children).style(
        Style::new()
            .width(aqua_ui::style::Length::Px(width))
            .height(aqua_ui::style::Length::Px(width / aspect)),
    );
    let Some((mode, split, emit)) = interactive else {
        return root;
    };
    let press = Rc::clone(&emit);
    let drag = Rc::clone(&emit);
    let release = Rc::clone(&emit);
    root.focusable(true)
        .cursor(if mode == Mode::Toggle {
            aqua_core::CursorIcon::Pointer
        } else {
            aqua_core::CursorIcon::ColResize
        })
        .on_input(InputKind::PointerDown, EventPhase::Bubble, move |event| {
            if !matches!(
                &event.input,
                RoutedInput::Native(InputEvent::PointerDown(_, PointerButton::Primary))
            ) {
                return InputResponse::default();
            }
            let Some(position) = event.local_position else {
                return InputResponse::default();
            };
            press(Action::Press {
                x: position.x,
                width: event.bounds.w,
                clicks: event.click_count.unwrap_or(1),
            });
            InputResponse {
                focus: FocusRequest::Focus,
                pointer_capture: CaptureRequest::Capture,
                stop_propagation: true,
                ..InputResponse::handled()
            }
        })
        .on_input(InputKind::PointerMove, EventPhase::Bubble, move |event| {
            if let Some(position) = event.local_position {
                drag(Action::Move {
                    x: position.x,
                    width: event.bounds.w,
                });
            }
            InputResponse::default()
        })
        .on_input(InputKind::PointerUp, EventPhase::Bubble, move |_| {
            release(Action::Release);
            InputResponse {
                pointer_capture: CaptureRequest::Release,
                ..InputResponse::default()
            }
        })
        .on_input(InputKind::KeyDown, EventPhase::Bubble, move |event| {
            let RoutedInput::Native(InputEvent::KeyDown(key)) = &event.input else {
                return InputResponse::default();
            };
            let changed = match (mode, key.key) {
                (
                    Mode::Toggle,
                    Key::Char(' ') | Key::Named(NamedKey::Enter | NamedKey::Left | NamedKey::Right),
                ) => true,
                (Mode::Slider | Mode::Blend, Key::Named(NamedKey::Left | NamedKey::Home)) => {
                    split > 0.0
                }
                (Mode::Slider | Mode::Blend, Key::Named(NamedKey::Right | NamedKey::End)) => {
                    split < 1.0
                }
                _ => false,
            };
            if !changed {
                return InputResponse::default();
            }
            emit(Action::Key {
                key: key.key,
                shift: key.modifiers.contains(aqua_core::Modifiers::SHIFT),
            });
            InputResponse {
                stop_propagation: true,
                ..InputResponse::handled()
            }
        })
}

#[cfg(test)]
#[path = "../../../../../tests/ui/rich/media/image_compare.rs"]
mod tests;
