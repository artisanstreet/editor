//! Attachment presentation owns frame geometry, preview activation and action placement.
use crate::theme::{TextStep, TypographyTokens};
use crate::{
    AssetId,
    button::{Button, ButtonVariant},
    media_frame::MediaFrame,
    motion::{MotionDuration, MotionPolicy, smooth_out},
    theme::ArtisanTheme,
};
use gpui::{
    AnyElement, App, IntoElement, ObjectFit, ParentElement as _, RenderImage, RenderOnce,
    SharedString, Styled as _, StyledImage as _, Window, canvas, div, img, px,
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

type Action = Box<dyn Fn(&mut Window, &mut App)>;
#[derive(IntoElement)]
pub struct AttachmentTile {
    id: SharedString,
    theme: ArtisanTheme,
    label: SharedString,
    image: Option<Arc<RenderImage>>,
    error: Option<String>,
    revealed: Option<Instant>,
    shaking: Option<Instant>,
    removable: bool,
    remove: Option<Action>,
    retry: Option<Action>,
    visible: Option<Box<dyn Fn(&mut App)>>,
}
impl AttachmentTile {
    pub fn new(
        id: impl Into<SharedString>,
        theme: ArtisanTheme,
        label: impl Into<SharedString>,
    ) -> Self {
        Self {
            id: id.into(),
            theme,
            label: label.into(),
            image: None,
            error: None,
            revealed: None,
            shaking: None,
            removable: true,
            remove: None,
            retry: None,
            visible: None,
        }
    }
    pub fn preview(mut self, image: Option<Arc<RenderImage>>, revealed: Option<Instant>) -> Self {
        self.image = image;
        self.revealed = revealed;
        self
    }
    pub fn error(mut self, error: Option<String>) -> Self {
        self.error = error;
        self
    }
    pub fn shaking(mut self, since: Option<Instant>) -> Self {
        self.shaking = since;
        self
    }
    pub fn removable(mut self, value: bool) -> Self {
        self.removable = value;
        self
    }
    /// Uses the shared frame's painted-visibility boundary for preview loading.
    pub fn on_visible(mut self, action: impl Fn(&mut App) + 'static) -> Self {
        self.visible = Some(Box::new(action));
        self
    }
    pub fn on_remove(mut self, action: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.remove = Some(Box::new(action));
        self
    }
    pub fn on_retry(mut self, action: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.retry = Some(Box::new(action));
        self
    }
}
const SHAKE: Duration = Duration::from_millis(280);
fn shake_offset(elapsed: Duration) -> f32 {
    const STOPS: [(f32, f32); 5] = [
        (0., 0.),
        (80. / 280., 6.),
        (160. / 280., -6.),
        (220. / 280., 4.),
        (1., 0.),
    ];
    let progress = (elapsed.as_secs_f32() / SHAKE.as_secs_f32()).clamp(0., 1.);
    STOPS
        .windows(2)
        .find(|leg| progress <= leg[1].0)
        .map_or(0., |leg| {
            let ((start, from), (end, to)) = (leg[0], leg[1]);
            from + (to - from) * smooth_out((progress - start) / (end - start))
        })
}
impl RenderOnce for AttachmentTile {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = self.theme;
        let reduced = MotionPolicy::current(cx) == MotionPolicy::Reduced;
        let reveal = self
            .revealed
            .filter(|_| !reduced)
            .map(|since| {
                since.elapsed().as_secs_f32() / MotionDuration::Medium.as_duration().as_secs_f32()
            })
            .filter(|progress| *progress < 1.);
        let shake = self
            .shaking
            .filter(|_| !reduced)
            .map(|since| since.elapsed())
            .filter(|elapsed| *elapsed < SHAKE);
        let preview = self.image.clone();
        let failed = self.error.is_some();
        let content: AnyElement = if let Some(error) = self.error {
            div()
                .size_full()
                .p(px(6.))
                .text_size(TypographyTokens::size(TextStep::X2s))
                .text_color(theme.colors.destructive.to_paint())
                .child(format!("{} failed: {error}", self.label))
                .into_any_element()
        } else if let Some(image) = self.image {
            let picture = img(image).size_full().object_fit(ObjectFit::Cover);
            if let Some(progress) = reveal {
                div()
                    .absolute()
                    .inset_0()
                    .blur(px(10. * (1. - smooth_out(progress))))
                    .child(picture)
                    .into_any_element()
            } else {
                picture.into_any_element()
            }
        } else {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .px(px(5.))
                .text_size(TypographyTokens::size(TextStep::X2s))
                .text_color(theme.colors.muted_foreground.to_paint())
                .child("Preparing…")
                .into_any_element()
        };
        let mut content = div().relative().size_full().child(content);
        if let Some(action) = self.retry {
            content = content.child(
                div().absolute().bottom(px(2.)).left(px(2.)).child(
                    Button::icon(
                        "retry",
                        theme,
                        AssetId::TABLER_REFRESH,
                        "Retry image preview",
                    )
                    .variant(ButtonVariant::Ghost)
                    .on_activate(move |_, window, cx| {
                        cx.stop_propagation();
                        action(window, cx);
                    }),
                ),
            );
        }
        if let Some(action) = self.remove {
            content = content.child(
                div().absolute().top(px(2.)).right(px(2.)).child(
                    Button::icon("remove", theme, AssetId::TABLER_X, "Remove image")
                        .disabled(!self.removable)
                        .variant(ButtonVariant::Ghost)
                        .on_activate(move |_, window, cx| {
                            cx.stop_propagation();
                            action(window, cx);
                        }),
                ),
            );
        }
        if reveal.is_some() || shake.is_some() {
            content = content.child(
                canvas(
                    |_, _, _| {},
                    |_, (), window, _| window.request_animation_frame(),
                )
                .absolute()
                .size_0(),
            );
        }
        let mut frame = MediaFrame::attachment(self.id.clone(), theme, content)
            .debug_selector(self.id.clone())
            .error(failed);
        if let Some(visible) = self.visible {
            frame = frame.on_visible(visible);
        }
        if let Some(image) = preview {
            frame = frame.on_activate(
                self.id,
                format!("Open {} preview", self.label),
                move |_, window, cx| crate::image_preview::open(image.clone(), window, cx),
            );
        }
        let mut placed = div().relative().flex_none().child(frame);
        if let Some(elapsed) = shake {
            placed = placed.left(px(shake_offset(elapsed)));
        }
        placed
    }
}
