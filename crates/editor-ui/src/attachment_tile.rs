//! Attachment presentation owns frame geometry, preview activation and action placement.
use crate::theme::{RadiusTokens, TextStep, TypographyTokens};
use crate::{
    AssetId,
    button::{Button, ButtonVariant},
    media_frame::MediaFrame,
    motion::{AnimatedPaint, MotionDuration, MotionPolicy, smooth_out},
    theme::ArtisanTheme,
};
use gpui::{
    AnyElement, App, Corners, Filter, IntoElement, ObjectFit, ParentElement as _, RenderImage,
    RenderOnce, SharedString, Styled as _, StyledImage as _, Window, canvas, div, img, px,
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

type Action = Box<dyn Fn(&mut Window, &mut App)>;
type Visible = Box<dyn Fn(&mut App)>;
#[derive(IntoElement)]
pub struct AttachmentTile {
    id: SharedString,
    theme: ArtisanTheme,
    label: SharedString,
    image: Option<Arc<RenderImage>>,
    /// A file or folder: its glyph over the name, in place of a picture.
    glyph: Option<AssetId>,
    error: Option<String>,
    revealed: Option<Instant>,
    preparing: bool,
    shaking: Option<Instant>,
    removable: bool,
    remove: Option<Action>,
    retry: Option<Action>,
    visible: Option<Visible>,
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
            glyph: None,
            error: None,
            revealed: None,
            preparing: false,
            shaking: None,
            removable: true,
            remove: None,
            retry: None,
            visible: None,
        }
    }
    #[must_use]
    pub fn preview(mut self, image: Option<Arc<RenderImage>>, revealed: Option<Instant>) -> Self {
        self.image = image;
        self.revealed = revealed;
        self
    }
    /// Shows a file or folder by `glyph` and name rather than a picture.
    #[must_use]
    pub const fn file(mut self, glyph: AssetId) -> Self {
        self.glyph = Some(glyph);
        self
    }
    /// Conversion or upload is still running: the preview stays blurred
    /// and sharpens from `revealed` once it is done.
    #[must_use]
    pub const fn preparing(mut self, value: bool) -> Self {
        self.preparing = value;
        self
    }
    #[must_use]
    pub fn error(mut self, error: Option<String>) -> Self {
        self.error = error;
        self
    }
    #[must_use]
    pub const fn shaking(mut self, since: Option<Instant>) -> Self {
        self.shaking = since;
        self
    }
    #[must_use]
    pub const fn removable(mut self, value: bool) -> Self {
        self.removable = value;
        self
    }
    /// Uses the shared frame's painted-visibility boundary for preview loading.
    #[must_use]
    pub fn on_visible(mut self, action: impl Fn(&mut App) + 'static) -> Self {
        self.visible = Some(Box::new(action));
        self
    }
    #[must_use]
    pub fn on_remove(mut self, action: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.remove = Some(Box::new(action));
        self
    }
    #[must_use]
    pub fn on_retry(mut self, action: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.retry = Some(Box::new(action));
        self
    }
}
const SHAKE: Duration = Duration::from_millis(280);
/// The preparing blur; the reveal sharpens out of it.
const PREPARING_BLUR: f32 = 10.;
/// Where the remove button sits inside the tile's border.
pub(crate) const ACTION_INSET: f32 = 2.;
/// The remove card: 4 px around its 16 px glyph.
pub(crate) const REMOVE_SIZE: f32 = 24.;
fn reveal_progress(since: Instant) -> f32 {
    since.elapsed().as_secs_f32() / MotionDuration::Medium.as_duration().as_secs_f32()
}
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
            (to - from).mul_add(smooth_out((progress - start) / (end - start)), from)
        })
}
impl RenderOnce for AttachmentTile {
    #[expect(
        clippy::too_many_lines,
        reason = "one element tree; splitting it would only thread its locals through helpers"
    )]
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = self.theme;
        let reduced = MotionPolicy::current(cx) == MotionPolicy::Reduced;
        let reveal = self
            .revealed
            .filter(|since| !reduced && reveal_progress(*since) < 1.);
        let shake = self
            .shaking
            .filter(|_| !reduced)
            .map(|since| since.elapsed())
            .filter(|elapsed| *elapsed < SHAKE);
        let preview = self.image.clone();
        let failed = self.error.is_some();
        let radius = MediaFrame::attachment_content_radius();
        let corners = Corners::all(radius);
        let content: AnyElement = if let Some(error) = self.error {
            div()
                .size_full()
                .p(px(6.))
                .text_size(TypographyTokens::size(TextStep::X2s))
                .text_color(theme.colors.destructive_text.to_paint())
                .child(format!("{} failed: {error}", self.label))
                .into_any_element()
        } else if let Some(image) = self.image {
            let picture = img(image)
                .size_full()
                .rounded(radius)
                .object_fit(ObjectFit::Cover);
            if self.preparing {
                AnimatedPaint::new(
                    div().absolute().inset_0().child(picture),
                    move |bounds, recording, window, _| {
                        let blur = Filter::Blur(px(PREPARING_BLUR));
                        window.with_filter_layer(bounds, corners, &[blur], |window| {
                            window.paint_recording(recording);
                        });
                        false
                    },
                )
                .into_any_element()
            } else if let Some(since) = reveal {
                // The unblur is paint-only: it repaints without rendering again.
                AnimatedPaint::new(
                    div().absolute().inset_0().child(picture),
                    move |bounds, recording, window, cx| {
                        let progress = reveal_progress(since);
                        if progress >= 1. || cx.reduce_motion() {
                            window.paint_recording(recording);
                            return false;
                        }
                        let blur = Filter::Blur(px(PREPARING_BLUR * (1. - smooth_out(progress))));
                        window.with_filter_layer(bounds, corners, &[blur], |window| {
                            window.paint_recording(recording);
                        });
                        true
                    },
                )
                .into_any_element()
            } else {
                picture.into_any_element()
            }
        } else if let Some(glyph) = self.glyph {
            div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(4.))
                .px(px(6.))
                .child(
                    crate::asset_seam::asset_glyph(glyph)
                        .size(px(24.))
                        .text_color(theme.colors.text_muted.to_paint()),
                )
                .child(
                    div()
                        .w_full()
                        .text_center()
                        .truncate()
                        .text_size(TypographyTokens::size(TextStep::X2s))
                        .text_color(theme.colors.text.to_paint())
                        .child(self.label.clone()),
                )
                .into_any_element()
        } else {
            div().into_any_element()
        };
        let remove_label = if self.glyph.is_some() {
            "Remove file"
        } else {
            "Remove image"
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
                div()
                    .absolute()
                    .top(px(ACTION_INSET))
                    .right(px(ACTION_INSET))
                    .child(
                        Button::icon("remove", theme, AssetId::TABLER_X, remove_label)
                            .disabled(!self.removable)
                            .variant(ButtonVariant::Ghost)
                            .hover_fill()
                            .muted()
                            .glass()
                            .compact(px(REMOVE_SIZE))
                            .nested_radius(RadiusTokens::nested(radius, px(ACTION_INSET)))
                            .on_activate(move |_, window, cx| {
                                cx.stop_propagation();
                                action(window, cx);
                            }),
                    ),
            );
        }
        if shake.is_some() {
            content = content.child(
                canvas(
                    |_, _, _| {},
                    |_, (), window, _| window.request_animation_frame(),
                )
                .absolute()
                .size_0(),
            );
        }
        let mut frame = MediaFrame::attachment(self.id.clone(), &theme, content)
            .debug_selector(self.id.clone())
            .error(failed);
        if let Some(visible) = self.visible {
            frame = frame.on_visible(visible);
        }
        if let Some(image) = preview {
            frame = frame.on_activate(
                self.id,
                format!("Open {} preview", self.label),
                move |_, window, cx| crate::image_preview::open(Arc::clone(&image), window, cx),
            );
        }
        let mut placed = div().relative().flex_none().child(frame);
        if let Some(elapsed) = shake {
            placed = placed.left(px(shake_offset(elapsed)));
        }
        placed
    }
}
