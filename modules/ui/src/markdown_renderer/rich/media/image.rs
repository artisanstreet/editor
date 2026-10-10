//! `image`: one image, or a few side by side, shown as they are.
//!
//! An image keeps its own aspect ratio and pixel width, capped in height,
//! with a hairline edge so dark images hold their shape on the transcript.
//! Several images wrap in a row at a smaller cap. An image that cannot load
//! is one quiet line naming it. Right-clicking an image offers its path.
//!
//! [`ImageShowcase`] is the same view outside a fence: the transcript draws
//! images an engine generated or looked at with it.

use gpui::{
    AnyElement, App, ElementId, IntoElement, MouseButton, ParentElement as _, RenderOnce,
    SharedString, Styled as _, Window, div, prelude::*, px,
};
use serde::Deserialize;

use super::super::actions::{CopyEntry, show_copy_menu};
use super::super::kit;
use super::super::{BlockContext, RichBlock};
use super::picture::{self, Loaded, MAX_FRAME_HEIGHT, SizeHint};
use crate::theme::ArtisanTheme;

/// The height cap of each image when several share a row.
const GALLERY_HEIGHT: f32 = 240.0;

/// The `image` spec: one `src`, or `images`.
#[derive(Debug, Deserialize)]
#[serde(try_from = "Spec")]
pub(super) struct ImageBlock {
    images: Vec<Picture>,
    hint: SizeHint,
}

#[derive(Debug, Deserialize)]
struct Spec {
    #[serde(default)]
    src: Option<String>,
    #[serde(default)]
    alt: Option<String>,
    #[serde(default)]
    images: Vec<Picture>,
    #[serde(flatten)]
    hint: SizeHint,
}

#[derive(Clone, Debug, Deserialize)]
struct Picture {
    src: String,
    #[serde(default)]
    alt: Option<String>,
    #[serde(default)]
    caption: Option<String>,
}

impl TryFrom<Spec> for ImageBlock {
    type Error = String;

    fn try_from(spec: Spec) -> Result<Self, String> {
        let mut images = spec.images;
        if let Some(src) = spec.src {
            images.insert(
                0,
                Picture {
                    src,
                    alt: spec.alt,
                    caption: None,
                },
            );
        }
        images.retain(|image| !image.src.trim().is_empty());
        if images.is_empty() {
            return Err("an image needs a `src` or `images`".to_owned());
        }
        Ok(Self {
            images,
            hint: spec.hint,
        })
    }
}

impl RichBlock for ImageBlock {
    fn render(&self, cx: &BlockContext<'_>) -> AnyElement {
        let gallery = self.images.len() > 1;
        let views = self
            .images
            .iter()
            .enumerate()
            .map(|(index, image)| ImageShowcase {
                id: cx.key(format_args!("image-{index}")),
                src: SharedString::from(image.src.trim().to_owned()),
                caption: image
                    .caption
                    .as_deref()
                    .map(str::trim)
                    .filter(|caption| !caption.is_empty())
                    .map(|caption| SharedString::from(caption.to_owned())),
                theme: *cx.theme,
                hint: if gallery {
                    SizeHint::default()
                } else {
                    self.hint
                },
                max_height: if gallery {
                    GALLERY_HEIGHT
                } else {
                    MAX_FRAME_HEIGHT
                },
            });
        div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_wrap()
            .items_start()
            .gap(px(8.0))
            .children(views)
            .into_any_element()
    }

    fn markdown(&self) -> String {
        self.images
            .iter()
            .map(|image| {
                let alt = image.alt.as_deref().or(image.caption.as_deref());
                picture::markdown(alt.unwrap_or_default(), &image.src)
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

/// One image shown as it is: an engine's generated or viewed image, or an
/// `image` block's.
#[derive(IntoElement)]
pub struct ImageShowcase {
    id: SharedString,
    src: SharedString,
    caption: Option<SharedString>,
    theme: ArtisanTheme,
    hint: SizeHint,
    max_height: f32,
}

impl ImageShowcase {
    /// The image at `src`: an absolute path, a `file://` URI, an `http(s)`
    /// URL, or a path relative to the media root. `id` must be unique and
    /// stable across frames.
    pub fn new(
        id: impl Into<SharedString>,
        src: impl Into<SharedString>,
        theme: ArtisanTheme,
    ) -> Self {
        Self {
            id: id.into(),
            src: src.into(),
            caption: None,
            theme,
            hint: SizeHint::default(),
            max_height: MAX_FRAME_HEIGHT,
        }
    }

    /// A muted line under the image.
    #[must_use]
    pub fn caption(mut self, caption: impl Into<SharedString>) -> Self {
        self.caption = Some(caption.into());
        self
    }
}

impl RenderOnce for ImageShowcase {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = self.theme;
        let loaded = picture::load(window, cx, &self.id, &self.src);
        if matches!(loaded, Loaded::Failed) {
            return picture::unavailable(&theme, &self.src).into_any_element();
        }
        let (aspect, width) = Loaded::frame(&[&loaded], self.hint, self.max_height);
        let path = self.src.clone();
        div()
            .min_w_0()
            .max_w(width)
            .w_full()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(
                div()
                    .id(ElementId::Name(self.id.clone()))
                    .w_full()
                    .aspect_ratio(aspect)
                    .rounded(kit::inner_radius())
                    .overflow_hidden()
                    .border_1()
                    .border_color(kit::hairline(&theme))
                    .on_mouse_down(MouseButton::Right, move |event, window, cx| {
                        cx.stop_propagation();
                        let entries = vec![CopyEntry::primary("Copy Image Path", path.clone())];
                        show_copy_menu(event.position, entries, window, cx);
                    })
                    .child(loaded.element(&theme, &self.src)),
            )
            .children(self.caption.map(|caption| kit::muted(&theme, caption)))
            .into_any_element()
    }
}
