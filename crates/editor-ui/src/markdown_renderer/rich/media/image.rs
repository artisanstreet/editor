//! `image`: centered images over blurred, frame-filling copies.
//!
//! An image keeps its own aspect ratio inside a wide, height-capped frame,
//! with the shared neutral card shadow to lift it from the transcript.
//! Several images form balanced rows of two or three square tiles. An image
//! that cannot load is one quiet line naming it. Right-click offers its path.
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

/// The `image` spec: one `src`, or `images` (`paths` is an alias).
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
    #[serde(default, alias = "paths")]
    images: Vec<PictureInput>,
    #[serde(flatten)]
    hint: SizeHint,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum PictureInput {
    Path(MediaSource),
    Detailed(Picture),
}

impl From<PictureInput> for Picture {
    fn from(input: PictureInput) -> Self {
        match input {
            PictureInput::Path(src) => Self {
                src,
                alt: None,
                caption: None,
            },
            PictureInput::Detailed(picture) => picture,
        }
    }
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
        let mut images: Vec<Picture> = spec.images.into_iter().map(Picture::from).collect();
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

/// Avoid an orphan tile: seven images, for example, become 2 + 2 + 3.
fn gallery_rows(mut count: usize) -> Vec<u16> {
    let mut rows = Vec::new();
    while count > 0 {
        let columns = match count {
            1 => 1,
            2 | 4 => 2,
            _ if count.is_multiple_of(3) => 3,
            _ => 2,
        };
        rows.push(columns);
        count -= usize::from(columns);
    }
    rows
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
        let loader = gallery.then(|| {
            let sources: Vec<_> = self.images.iter().map(|image| image.src.clone()).collect();
            let owners: Vec<_> = (0..sources.len())
                .map(|index| cx.key(format_args!("image-{index}")))
                .collect();
            let media = cx.media.cloned();
            std::rc::Rc::new(move |index: usize, window: &mut Window, cx: &mut App| {
                match picture::load(window, cx, &owners[index], &sources[index], media.as_ref()) {
                    Loaded::Ready(image) => crate::overlay::PreviewLoad::Ready(image),
                    Loaded::Loading => crate::overlay::PreviewLoad::Loading,
                    Loaded::Failed => crate::overlay::PreviewLoad::Failed,
                }
            }) as std::rc::Rc<crate::overlay::GalleryLoader>
        });
        let mut views = self
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
                gallery,
                preview_gallery: loader.as_ref().map(|loader| crate::overlay::ImageGallery {
                    index,
                    count: self.images.len(),
                    load: std::rc::Rc::clone(loader),
                    forward: true,
                }),
            });
        let mut root = div().w_full().min_w_0().flex().flex_col().gap(px(8.0));
        if gallery {
            for columns in gallery_rows(self.images.len()) {
                root = root.child(
                    div()
                        .w_full()
                        .min_w_0()
                        .grid()
                        .grid_cols(columns)
                        .items_start()
                        .gap(px(8.0))
                        .children(views.by_ref().take(usize::from(columns))),
                );
            }
        } else {
            root = root.children(views);
        }
        root.into_any_element()
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
    gallery: bool,
    preview_gallery: Option<crate::overlay::ImageGallery>,
}

impl RenderOnce for ImageShowcase {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = self.theme;
        let loaded = picture::load(window, cx, &self.id, &self.src, self.media.as_ref());
        let (aspect, _) = Loaded::frame(&[&loaded], self.hint, MAX_FRAME_HEIGHT);
        let aspect = if self.gallery {
            1.0
        } else {
            aspect.max(16.0 / 9.0)
        };
        let width = px(MAX_FRAME_HEIGHT * aspect);
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
            .child(match &loaded {
                Loaded::Ready(image) => {
                    crate::media_frame::MediaFrame::image_content(image).into_any_element()
                }
                Loaded::Loading | Loaded::Failed => loaded.element(&theme, &self.src),
            });
        let mut frame = crate::media_frame::MediaFrame::new(theme, aspect, content)
            .debug_selector(self.id.clone());
        if let Some(image) = preview {
            let gallery = self.preview_gallery;
            frame = frame.on_activate(
                ElementId::Name(self.id.clone()),
                format!("{}. Open image preview", self.alt),
                move |_, window, cx| {
                    if let Some(gallery) = &gallery {
                        crate::overlay::show_gallery(
                            std::sync::Arc::clone(&image),
                            gallery.clone(),
                            window,
                            cx,
                        );
                    } else {
                        crate::image_preview::open(std::sync::Arc::clone(&image), window, cx);
                    }
                },
            );
        }
        div()
            .min_w_0()
            .when(!self.gallery, |frame| frame.max_w(width))
            .w_full()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(frame)
            .children(self.caption.map(|caption| kit::muted(&theme, caption)))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, Render, TestAppContext};

    struct Gallery(ImageBlock);
    impl Render for Gallery {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w_full().child(self.0.render(&BlockContext {
                theme: &ArtisanTheme::resolved(),
                media: None,
                id: "gallery".into(),
            }))
        }
    }

    #[gpui::test]
    fn image_gallery_tiles_fill_balanced_rows_at_narrow_and_wide_sizes(cx: &mut TestAppContext) {
        let (_, cx) = cx.add_window_view(|_, _| {
            Gallery(
                serde_json::from_str(r#"{"paths": ["a.png", "b.png", "c.png", "d.png", "e.png"]}"#)
                    .unwrap(),
            )
        });
        for width in [600.0, 280.0] {
            cx.simulate_resize(gpui::size(px(width), px(800.0)));
            cx.update(|window, cx| window.draw(cx).clear(cx));
            let tiles = [
                "gallery-image-0",
                "gallery-image-1",
                "gallery-image-2",
                "gallery-image-3",
                "gallery-image-4",
            ]
            .map(|selector| cx.debug_bounds(selector).unwrap());
            for tile in tiles {
                assert!((tile.size.width - tile.size.height).abs() < px(1.0));
                assert!(tile.left() >= px(0.0) && tile.right() <= px(width + 1.0));
            }
            assert_eq!(tiles[0].top(), tiles[1].top());
            assert_eq!(tiles[2].top(), tiles[4].top());
            assert!(tiles[2].top() >= tiles[0].bottom() + px(7.0));
            assert!((tiles[1].right() - px(width)).abs() < px(1.0));
            assert!((tiles[4].right() - px(width)).abs() < px(1.0));
            assert!(tiles[0].size.width > tiles[2].size.width);
        }
    }
}
