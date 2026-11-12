use std::rc::Rc;
use std::time::Duration;

use aqua_core::{Transform2D, image::ImageFit};
use aqua_ui::{
    AlignItems, JustifyContent,
    element::{Box as UiBox, Button as UiButton, Element, Key, Media},
    style::{Length, Style},
};
use editor_assets::AssetId;

use super::{
    kit::{
        Finish, IconButton, IconButtonSize, IconSize, Space, Spaced, TextRole,
        control::icon_button::CORNER_INSET,
    },
    media_frame::MediaFrame,
    motion::smooth_out,
    theme::ArtisanTheme,
};

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
        let failed = self.error.is_some();
        let caption = |words: String, color: crate::ui::theme::Oklch| {
            TextRole::Caption
                .text(&theme, words)
                .gpui_source_color(color.to_reference_text_color())
        };
        let content = if let Some(error) = self.error {
            UiBox::new()
                .padded(Space::S6)
                .child(
                    caption(
                        format!("{} failed: {error}", self.label),
                        theme.colors.destructive_text,
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
                .style(MediaFrame::thumb_content_style());
            if blur > 0.0 {
                image = image.style(Style::new().content_blur(blur));
            }
            image.build()
        } else if let Some(glyph) = self.glyph {
            UiBox::new()
                .layout("column")
                .spaced(Space::S4)
                .padded(Space::S6)
                .width(Length::Pct(100.0))
                .height(Length::Pct(100.0))
                .style(
                    Style::new()
                        .align(AlignItems::CENTER)
                        .justify(JustifyContent::CENTER),
                )
                .child(IconSize::Lg.glyph(glyph, theme.colors.text_muted.to_reference_paint()))
                .child(
                    caption(self.label.clone(), theme.colors.text)
                        .style(Style::new().text_wrap(false))
                        .build(),
                )
                .build()
        } else {
            UiBox::new().build()
        };
        let mut layers = UiBox::new()
            .width(Length::Pct(100.0))
            .height(Length::Pct(100.0));
        if let Some(action) = self.activate.filter(|_| self.image.is_some()) {
            let button = UiButton::new()
                .width(Length::Pct(100.0))
                .height(Length::Pct(100.0))
                .on_activate(move || action())
                .child(content)
                .build()
                .aria_label(format!("Open {} preview", self.label));
            layers = layers.child(button);
        } else {
            layers = layers.child(content);
        }
        let radius = MediaFrame::thumb_radius();
        if let Some(action) = self.retry {
            let retry = IconButton::new(
                "retry",
                theme,
                AssetId::TABLER_REFRESH,
                "Retry image preview",
                IconButtonSize::Sm,
                Finish::Plain,
            )
            .nested(radius, CORNER_INSET)
            .on_activate(move |_| action())
            .build()
            .style(Style::new().absolute_at(Some(CORNER_INSET), None, None, Some(CORNER_INSET)));
            layers = layers.child(retry);
        }
        if let Some(action) = self.remove {
            let label = if self.glyph.is_some() {
                "Remove file"
            } else {
                "Remove image"
            };
            let remove = IconButton::corner_close("remove", theme, label, radius)
                .disabled(!self.removable)
                .on_activate(move |_| action())
                .build()
                .style(Style::new().absolute_at(
                    None,
                    Some(CORNER_INSET),
                    Some(CORNER_INSET),
                    None,
                ));
            layers = layers.child(remove);
        }
        let mut element = MediaFrame::attachment(self.id, theme, layers.build())
            .error(failed)
            .build();
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
