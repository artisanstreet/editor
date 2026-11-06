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
                .gap(8.0)
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
            .gap(8.0)
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
        .gap(if status.is_some() { 4.0 } else { 6.0 })
        .style(
            Style::new()
                .absolute_at(Some(0.0), Some(0.0), None, None)
                .align(AlignItems::CENTER)
                .justify(JustifyContent::CENTER)
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
            .style(
                Style::new()
                    .min_width(Length::Px(0.0))
                    .max_width(Length::Pct(100.0)),
            )
            .build()
    };
    if let Some(error) = status {
        panel = panel
            .child(crate::ui::asset::source_asset_glyph(
                editor_assets::AssetId::TABLER_FILE_OFF,
                20.0,
                theme.colors.text_muted.to_reference_paint(),
            ))
            .child(text(error.title().to_owned(), theme.colors.text))
            .child(text(source.trim().to_owned(), theme.colors.text_muted));
        if error != Failure::Invalid {
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

fn pill(theme: &crate::ui::theme::ArtisanTheme, text: String) -> aqua_ui::element::Element {
    use aqua_text::{FontFamily, FontWeight, TextStyle};
    use aqua_ui::{
        AlignItems,
        element::{Box as UiBox, Text},
        style::Style,
    };
    let radius = 11.0;
    let border = theme
        .colors
        .text
        .at(crate::ui::theme::Opacity::GlassBorder)
        .to_reference_paint();
    let shadows = crate::ui::glass::glass_card_shadows(theme, crate::ui::theme::Elevation::Overlay)
        .into_iter()
        .map(Into::into)
        .collect();
    UiBox::new()
        .height(22.0)
        .radius(radius)
        .clip(true)
        .style(
            Style::new()
                .gpui_border(aqua_core::Insets::uniform(1.0), border, false)
                .gpui_shadows(shadows)
                .align(AlignItems::CENTER)
                .padding_insets(aqua_core::Insets {
                    left: 9.0,
                    right: 9.0,
                    top: 0.0,
                    bottom: 0.0,
                }),
        )
        .child(crate::ui::glass::glass_layers(
            theme,
            crate::ui::glass::GlassStrength::Quiet,
            radius,
        ))
        .child(
            Text::new(text)
                .text_style(TextStyle {
                    families: vec![FontFamily::Named(theme.typography.sans.family.to_owned())],
                    size_px: 12.0,
                    line_height_px: 16.0,
                    weight: FontWeight::MEDIUM,
                    ..TextStyle::default()
                })
                .gpui_source_color(theme.colors.text.to_reference_text_color())
                .wrap(false)
                .build(),
        )
        .build()
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
            if left { Some(8.0) } else { None },
            Some(8.0),
            if left { None } else { Some(8.0) },
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
    use crate::ui::{asset::source_asset_glyph, theme::Opacity};
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
    let handle = UiBox::new()
        .width(32.0)
        .height(32.0)
        .radius(16.0)
        .clip(true)
        .style(
            Style::new()
                .absolute_at(Some(x - 16.0), Some(height / 2.0 - 16.0), None, None)
                .gpui_border(
                    aqua_core::Insets::uniform(1.0),
                    theme
                        .colors
                        .text
                        .at(Opacity::GlassBorder)
                        .to_reference_paint(),
                    false,
                )
                .gpui_shadows(
                    crate::ui::glass::glass_card_shadows(
                        theme,
                        crate::ui::theme::Elevation::Overlay,
                    )
                    .into_iter()
                    .map(Into::into)
                    .collect(),
                )
                .align(AlignItems::CENTER)
                .justify(JustifyContent::CENTER),
        )
        .child(crate::ui::glass::glass_layers(
            theme,
            crate::ui::glass::GlassStrength::Quiet,
            16.0,
        ))
        .child(source_asset_glyph(
            editor_assets::AssetId::TABLER_CHEVRON_LEFT,
            12.0,
            theme.colors.text.to_reference_paint(),
        ))
        .child(source_asset_glyph(
            editor_assets::AssetId::TABLER_CHEVRON_RIGHT,
            12.0,
            theme.colors.text.to_reference_paint(),
        ))
        .build();
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
    use crate::ui::{glass::card_shadows, theme::RadiusStep, theme::RadiusTokens};
    use aqua_ui::{element::Box as UiBox, input::EventPhase, style::Style};
    let radius = RadiusTokens::value(RadiusStep::Xl);
    let root = UiBox::new()
        .width(width)
        .height(width / aspect)
        .radius(radius)
        .clip(true)
        .style(Style::new().gpui_shadows(card_shadows(theme).into_iter().map(Into::into).collect()))
        .child(base)
        .children(layers)
        .build();
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
