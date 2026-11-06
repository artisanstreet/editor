use std::rc::Rc;
use std::time::Duration;

use aqua_core::{Insets, Transform2D, image::ImageFit};
use aqua_text::{FontFamily, FontWeight, TextStyle};
use aqua_ui::{
    AlignItems, JustifyContent,
    element::{Box as UiBox, Button as UiButton, Element, Key, Media, Text},
    style::{Length, Style},
};
use editor_assets::AssetId;

use super::{
    asset::source_asset_glyph,
    button::{Button, ButtonVariant},
    glass::card_shadows,
    motion::smooth_out,
    theme::{ArtisanTheme, Opacity},
};

pub const ACTION_INSET: f32 = 2.0;
pub const REMOVE_SIZE: f32 = 24.0;
const PREPARING_BLUR: f32 = 10.0;
const SHAKE: Duration = Duration::from_millis(280);

pub struct AttachmentTile {
    id: Key,
    theme: ArtisanTheme,
    label: String,
    image: Option<aqua::ImageHandle>,
    glyph: Option<AssetId>,
    error: Option<String>,
    reveal_elapsed: Option<Duration>,
    preparing: bool,
    shake_elapsed: Option<Duration>,
    reduced_motion: bool,
    removable: bool,
    remove: Option<Rc<dyn Fn()>>,
    retry: Option<Rc<dyn Fn()>>,
    activate: Option<Rc<dyn Fn()>>,
}

impl AttachmentTile {
    pub fn new(id: impl Into<Key>, theme: ArtisanTheme, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            theme,
            label: label.into(),
            image: None,
            glyph: None,
            error: None,
            reveal_elapsed: None,
            preparing: false,
            shake_elapsed: None,
            reduced_motion: false,
            removable: true,
            remove: None,
            retry: None,
            activate: None,
        }
    }
    pub fn preview(mut self, image: Option<aqua::ImageHandle>, elapsed: Option<Duration>) -> Self {
        self.image = image;
        self.reveal_elapsed = elapsed;
        self
    }
    pub const fn file(mut self, glyph: AssetId) -> Self {
        self.glyph = Some(glyph);
        self
    }
    pub const fn preparing(mut self, preparing: bool) -> Self {
        self.preparing = preparing;
        self
    }
    pub fn error(mut self, error: Option<String>) -> Self {
        self.error = error;
        self
    }
    pub const fn shaking(mut self, elapsed: Option<Duration>) -> Self {
        self.shake_elapsed = elapsed;
        self
    }
    pub const fn reduced_motion(mut self, reduced: bool) -> Self {
        self.reduced_motion = reduced;
        self
    }
    pub const fn removable(mut self, removable: bool) -> Self {
        self.removable = removable;
        self
    }
    pub fn on_remove(mut self, action: impl Fn() + 'static) -> Self {
        self.remove = Some(Rc::new(action));
        self
    }
    pub fn on_retry(mut self, action: impl Fn() + 'static) -> Self {
        self.retry = Some(Rc::new(action));
        self
    }
    pub fn on_activate(mut self, action: impl Fn() + 'static) -> Self {
        self.activate = Some(Rc::new(action));
        self
    }

    pub fn build(self) -> Element {
        let theme = self.theme;
        let text_style = TextStyle {
            families: vec![FontFamily::Named(theme.typography.sans.family.to_owned())],
            size_px: 11.0,
            line_height_px: 13.75,
            weight: FontWeight::NORMAL,
            ..TextStyle::default()
        };
        let failed = self.error.is_some();
        let content = if let Some(error) = self.error {
            UiBox::new()
                .padding(6.0)
                .child(
                    Text::new(format!("{} failed: {error}", self.label))
                        .style(
                            Style::new()
                                .text_style(text_style.clone())
                                .gpui_source_text_color(
                                    theme.colors.destructive_text.to_reference_text_color(),
                                ),
                        )
                        .build(),
                )
                .build()
        } else if let Some(image) = self.image.clone() {
            let blur = if self.preparing {
                PREPARING_BLUR
            } else if !self.reduced_motion {
                self.reveal_elapsed.map_or(0.0, |elapsed| {
                    PREPARING_BLUR * (1.0 - smooth_out((elapsed.as_secs_f32() / 0.35).min(1.0)))
                })
            } else {
                0.0
            };
            let mut image = Media::new(image)
                .width(Length::Pct(100.0))
                .height(Length::Pct(100.0))
                .fit(ImageFit::Cover)
                .radius(5.0);
            if blur > 0.0 {
                image = image.style(Style::new().content_blur(blur));
            }
            image.build()
        } else if let Some(glyph) = self.glyph {
            let icon =
                source_asset_glyph(glyph, 24.0, theme.colors.text_muted.to_reference_paint());
            let text = Text::new(self.label.clone())
                .style(
                    Style::new()
                        .text_style(text_style)
                        .gpui_source_text_color(theme.colors.text.to_reference_text_color())
                        .text_wrap(false),
                )
                .build();
            UiBox::new()
                .layout("column")
                .gap(4.0)
                .width(Length::Pct(100.0))
                .height(Length::Pct(100.0))
                .style(
                    Style::new()
                        .align(AlignItems::CENTER)
                        .justify(JustifyContent::CENTER)
                        .padding_insets(Insets {
                            left: 6.0,
                            right: 6.0,
                            top: 0.0,
                            bottom: 0.0,
                        }),
                )
                .child(icon)
                .child(text)
                .build()
        } else {
            UiBox::new().build()
        };
        let mut frame = UiBox::new()
            .width(72.0)
            .height(72.0)
            .radius(6.0)
            .clip(true)
            .style(
                Style::new()
                    .gpui_background(aqua_core::GpuiBackground::Solid(
                        theme.colors.surface.to_reference_paint(),
                    ))
                    .gpui_border(
                        Insets::uniform(1.0),
                        if failed {
                            theme.colors.destructive.to_reference_paint()
                        } else {
                            theme
                                .colors
                                .text
                                .at(Opacity::MediaEdge)
                                .to_reference_paint()
                        },
                        false,
                    )
                    .flex_shrink(0.0)
                    .gpui_shadows(
                        card_shadows(&theme)
                            .into_iter()
                            .map(aqua_ui::style::GpuiShadowLayer::from)
                            .collect(),
                    ),
            );
        if let Some(action) = self.activate.filter(|_| self.image.is_some()) {
            let button = UiButton::new()
                .width(Length::Pct(100.0))
                .height(Length::Pct(100.0))
                .on_activate(move || action())
                .child(content)
                .build()
                .aria_label(format!("Open {} preview", self.label));
            frame = frame.child(button);
        } else {
            frame = frame.child(content);
        }
        if let Some(action) = self.retry {
            let retry = Button::icon(
                "retry",
                theme,
                AssetId::TABLER_REFRESH,
                "Retry image preview",
            )
            .variant(ButtonVariant::Ghost)
            .on_activate(move |_| action())
            .build()
            .style(Style::new().absolute_at(Some(2.0), None, None, Some(2.0)));
            frame = frame.child(retry);
        }
        if let Some(action) = self.remove {
            let label = if self.glyph.is_some() {
                "Remove file"
            } else {
                "Remove image"
            };
            let remove = Button::icon("remove", theme, AssetId::TABLER_X, label)
                .disabled(!self.removable)
                .variant(ButtonVariant::Ghost)
                .hover_fill()
                .muted()
                .glass()
                .compact(REMOVE_SIZE)
                .nested_radius(3.0)
                .on_activate(move |_| action())
                .build()
                .style(Style::new().absolute_at(
                    None,
                    Some(ACTION_INSET),
                    Some(ACTION_INSET),
                    None,
                ));
            frame = frame.child(remove);
        }
        let mut element = frame.build().with_key(self.id);
        if !self.reduced_motion {
            if let Some(elapsed) = self.shake_elapsed.filter(|elapsed| *elapsed < SHAKE) {
                element = element.style(
                    Style::new().transform(Transform2D::translate(shake_offset(elapsed), 0.0)),
                );
            }
        }
        element
    }
}

fn shake_offset(elapsed: Duration) -> f32 {
    let stops: [(f32, f32); 5] = [
        (0.0, 0.0),
        (80.0 / 280.0, 6.0),
        (160.0 / 280.0, -6.0),
        (220.0 / 280.0, 4.0),
        (1.0, 0.0),
    ];
    let progress = (elapsed.as_secs_f32() / SHAKE.as_secs_f32()).clamp(0.0, 1.0);
    stops
        .windows(2)
        .find(|leg| progress <= leg[1].0)
        .map_or(0.0, |leg| {
            let ((start, from), (end, to)) = (leg[0], leg[1]);
            (to - from).mul_add(smooth_out((progress - start) / (end - start)), from)
        })
}
