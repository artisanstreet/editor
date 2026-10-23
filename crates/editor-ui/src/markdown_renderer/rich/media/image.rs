//! `image`: one image, or a few side by side, shown as they are.
//!
//! An image keeps its own aspect ratio and pixel width, capped in height,
//! with a hairline edge so dark images hold their shape on the transcript.
//! Several images wrap in a row at a smaller cap. An image that cannot load
//! is one quiet line naming it. Right-clicking an image offers its path.
//!
//! [`ImageShowcase`] is the internal prepared image view: the transcript draws
//! images an engine generated or looked at with it.

use crate::markdown_renderer::rich::budget;
use gpui::{
    AnyElement, App, ElementId, IntoElement, MouseButton, ParentElement as _, RenderOnce,
    SharedString, Styled as _, Window, div, prelude::*, px,
};
use serde::Deserialize;

use super::super::actions::{CopyEntry, show_copy_menu};
use super::super::kit;
use super::super::{BlockContext, RichBlock};
use super::picture::{self, Loaded, MAX_FRAME_HEIGHT, SizeHint};
use super::source::MediaSource;
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
    src: Option<MediaSource>,
    #[serde(default)]
    alt: Option<String>,
    #[serde(default)]
    images: Vec<Picture>,
    #[serde(flatten)]
    hint: SizeHint,
}

#[derive(Clone, Debug, Deserialize)]
struct Picture {
    src: MediaSource,
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
        if images.len() > 16 {
            return Err("image gallery preview supports at most 16 images".into());
        }
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
    fn resident_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + budget::vector(&self.images)
            + self
                .images
                .iter()
                .map(|image| {
                    image.src.resident_bytes()
                        + budget::optional(image.alt.as_ref())
                        + budget::optional(image.caption.as_ref())
                })
                .sum::<usize>()
    }

    fn render(&self, cx: &BlockContext<'_>) -> AnyElement {
        let gallery = self.images.len() > 1;
        let views = self
            .images
            .iter()
            .enumerate()
            .map(|(index, image)| ImageShowcase {
                id: cx.key(format_args!("image-{index}")),
                media: cx.media.cloned(),
                src: image.src.clone(),
                alt: image
                    .alt
                    .as_deref()
                    .filter(|alt| !alt.trim().is_empty())
                    .map_or_else(|| image.src.raw(), SharedString::from),
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
}

/// One image shown as it is: an engine's generated or viewed image, or an
/// `image` block's.
#[derive(IntoElement)]
pub(super) struct ImageShowcase {
    media: Option<super::source::MediaFetcher>,
    id: SharedString,
    src: MediaSource,
    alt: SharedString,
    caption: Option<SharedString>,
    theme: ArtisanTheme,
    hint: SizeHint,
    max_height: f32,
}

impl RenderOnce for ImageShowcase {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = self.theme;
        let loaded = picture::load(window, cx, &self.id, &self.src, self.media.as_ref());
        let (aspect, width) = Loaded::frame(&[&loaded], self.hint, self.max_height);
        let path = self.src.raw();
        let preview = match &loaded {
            Loaded::Ready(image) => Some(std::sync::Arc::clone(image)),
            Loaded::Loading | Loaded::Failed => None,
        };
        let content = div()
            .relative()
            .size_full()
            .id(kit::part(&self.id, "content"))
            .role(gpui::Role::Image)
            .aria_label(self.alt.clone())
            .on_mouse_down(MouseButton::Right, move |event, window, cx| {
                cx.stop_propagation();
                show_copy_menu(
                    event.position,
                    vec![CopyEntry::primary("Copy Image Path", path.clone())],
                    window,
                    cx,
                );
            })
            .child(loaded.element(&theme, &self.src));
        let mut frame = crate::media_frame::MediaFrame::new(theme, aspect, content)
            .debug_selector(self.id.clone());
        if let Some(image) = preview {
            frame = frame.on_activate(
                ElementId::Name(self.id.clone()),
                format!("{}. Open image preview", self.alt),
                crate::image_preview::open_on_click(image),
            );
        }
        div()
            .min_w_0()
            .max_w(width)
            .w_full()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(frame)
            .children(self.caption.map(|caption| kit::muted(&theme, caption)))
            .into_any_element()
    }
}
