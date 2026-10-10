//! Loading an image `src` for the image kinds.
//!
//! The `src` resolves through [`super::source`]: URLs and local files load
//! through GPUI's image loader, `data:` URLs and files fetched from the
//! Forge's machine through [`ImageBytesLoader`]. Every load is keyed by the
//! block that made it as well as the `src` ([`Owned`]), so a block keeps the
//! image it first loaded and a later block naming a rewritten file loads it
//! afresh. Either way the block loads
//! it through GPUI's asset cache before layout, so it knows the intrinsic
//! size: the frame takes the image's own aspect ratio and markers and
//! dividers land on the displayed image, not on letterboxing. Until the image loads (or when it
//! cannot) the frame uses the spec's `width`/`height` hints, else 16:9.

use std::sync::Arc;

use gpui::{
    App, Asset, Div, ImageAssetLoader, ImageCacheError, ImageFormat, ImageSource, IntoElement,
    ObjectFit, ParentElement as _, RenderImage, Resource, SharedString, SharedUri, Styled as _,
    StyledImage as _, SvgRenderer, Window, div, img, px,
};
use serde::Deserialize;

use super::super::kit;
use super::source::{Located, MediaFetcher, MediaRequest, Owned, locate};
use crate::theme::ArtisanTheme;
use artisan_assets::AssetId;

/// The aspect ratio of a frame whose image has no known size.
const DEFAULT_ASPECT: f32 = 16.0 / 9.0;

/// The tallest a frame grows before it narrows to keep its aspect.
pub(super) const MAX_FRAME_HEIGHT: f32 = 480.0;

/// Optional size hints a spec may give before its image loads.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq)]
pub(super) struct SizeHint {
    #[serde(default)]
    pub(super) width: Option<f32>,
    #[serde(default)]
    pub(super) height: Option<f32>,
}

impl SizeHint {
    fn aspect(self) -> Option<f32> {
        match (self.width, self.height) {
            (Some(width), Some(height)) if width > 0.0 && height > 0.0 => {
                Some((width / height).clamp(0.05, 20.0))
            }
            _ => None,
        }
    }
}

/// Where an image stands this frame.
pub(super) enum Loaded {
    Loading,
    Ready(Arc<RenderImage>),
    Failed,
}

impl Loaded {
    /// The image's intrinsic aspect ratio and width, once loaded.
    fn intrinsic(&self) -> Option<(f32, f32)> {
        let Self::Ready(image) = self else {
            return None;
        };
        let size = image.size(0);
        #[expect(clippy::cast_precision_loss, reason = "image sides are far below 2^24")]
        let (width, height) = (size.width.0 as f32, size.height.0 as f32);
        (width > 0.0 && height > 0.0).then(|| ((width / height).clamp(0.05, 20.0), width))
    }

    /// One frame for several images: the widest ratio among those loaded,
    /// so every image fits and the narrower ones letterbox; until any loads,
    /// the hint's ratio, else 16:9. Never taller than `max_height`.
    pub(super) fn frame(images: &[&Self], hint: SizeHint, max_height: f32) -> (f32, gpui::Pixels) {
        let widest = images
            .iter()
            .filter_map(|image| image.intrinsic())
            .max_by(|a, b| a.0.total_cmp(&b.0));
        let (aspect, natural) = match widest {
            Some((aspect, width)) => (aspect, Some(width)),
            None => (hint.aspect().unwrap_or(DEFAULT_ASPECT), hint.width),
        };
        let capped = max_height * aspect;
        let width = natural.map_or(capped, |natural| natural.min(capped));
        (aspect, px(width.max(48.0)))
    }

    /// The image filling its frame, or a quiet placeholder.
    pub(super) fn element(&self, theme: &ArtisanTheme, src: &str) -> gpui::AnyElement {
        match self {
            Self::Ready(image) => img(ImageSource::Render(Arc::clone(image)))
                .size_full()
                .object_fit(ObjectFit::Contain)
                .into_any_element(),
            Self::Loading => placeholder(theme, None, "Loading image…").into_any_element(),
            Self::Failed => placeholder(
                theme,
                Some(AssetId::TABLER_FILE_OFF),
                format!("Couldn't load {}", src.trim()),
            )
            .into_any_element(),
        }
    }
}

/// Loads `src` for the block part `owner` through the asset cache; the view
/// redraws when it lands.
pub(super) fn load(window: &mut Window, cx: &mut App, owner: &SharedString, src: &str) -> Loaded {
    let bytes = match locate(src, cx) {
        Located::Url(url) => {
            return load_resource(
                window,
                cx,
                &Owned::new(owner, Resource::Uri(SharedUri::from(url))),
            );
        }
        Located::Path(path) => {
            return load_resource(window, cx, &Owned::new(owner, Resource::Path(path.into())));
        }
        Located::Invalid => return Loaded::Failed,
        Located::Data(bytes) => Owned::new(owner, ImageBytes::Data(bytes.into())),
        Located::Fetched(request) => Owned::new(owner, ImageBytes::Fetched(request)),
    };
    match window.use_asset::<ImageBytesLoader>(&bytes, cx) {
        None => Loaded::Loading,
        Some(Some(image)) => Loaded::Ready(image),
        Some(None) => Loaded::Failed,
    }
}

fn load_resource(window: &mut Window, cx: &mut App, resource: &Owned<Resource>) -> Loaded {
    match window.use_asset::<ResourceLoader>(resource, cx) {
        None => Loaded::Loading,
        Some(Ok(image)) => Loaded::Ready(image),
        Some(Err(_)) => Loaded::Failed,
    }
}

/// GPUI's image loader for URLs and this machine's files, keyed per block.
enum ResourceLoader {}

impl Asset for ResourceLoader {
    type Source = Owned<Resource>;
    type Output = Result<Arc<RenderImage>, ImageCacheError>;

    fn load(
        Owned { source, .. }: Self::Source,
        cx: &mut App,
    ) -> impl std::future::Future<Output = Self::Output> + Send + 'static {
        ImageAssetLoader::load(source, cx)
    }
}

/// Image bytes that come from neither a URL nor this machine's disk.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum ImageBytes {
    /// A `data:` URL's payload.
    Data(Arc<[u8]>),
    /// A file fetched from where the open thread's files live.
    Fetched(MediaRequest),
}

/// Decodes [`ImageBytes`], fetching them through the host when they name a
/// file. A file that cannot be fetched or decoded yields `None`.
enum ImageBytesLoader {}

impl Asset for ImageBytesLoader {
    type Source = Owned<ImageBytes>;
    type Output = Option<Arc<RenderImage>>;

    fn load(
        Owned { source, .. }: Self::Source,
        cx: &mut App,
    ) -> impl std::future::Future<Output = Self::Output> + Send + 'static {
        let renderer = cx.svg_renderer();
        let fetch = match &source {
            ImageBytes::Fetched(request) => cx
                .try_global::<MediaFetcher>()
                .map(|fetcher| fetcher.fetch(request.clone())),
            ImageBytes::Data(_) => None,
        };
        async move {
            let bytes = match source {
                ImageBytes::Data(bytes) => bytes,
                ImageBytes::Fetched(_) => fetch?.await?,
            };
            decode(&bytes, renderer)
        }
    }
}

/// Decodes image bytes by their content, not by any name.
fn decode(bytes: &[u8], renderer: SvgRenderer) -> Option<Arc<RenderImage>> {
    let format = sniff(bytes)?;
    gpui::Image::from_bytes(format, bytes.to_vec())
        .to_image_data(renderer)
        .ok()
}

/// The image format `bytes` begin with.
pub(super) fn sniff(bytes: &[u8]) -> Option<ImageFormat> {
    let starts = |prefix: &[u8]| bytes.starts_with(prefix);
    Some(if starts(b"\x89PNG\r\n\x1a\n") {
        ImageFormat::Png
    } else if starts(b"\xff\xd8\xff") {
        ImageFormat::Jpeg
    } else if starts(b"GIF87a") || starts(b"GIF89a") {
        ImageFormat::Gif
    } else if bytes.len() >= 12 && starts(b"RIFF") && &bytes[8..12] == b"WEBP" {
        ImageFormat::Webp
    } else if starts(b"BM") {
        ImageFormat::Bmp
    } else if starts(b"\x00\x00\x01\x00") {
        ImageFormat::Ico
    } else if starts(b"II*\x00") || starts(b"MM\x00*") {
        ImageFormat::Tiff
    } else if is_svg(bytes) {
        ImageFormat::Svg
    } else {
        return None;
    })
}

/// SVG is text: an `<svg` element within the first kilobyte.
fn is_svg(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(1024)];
    let text = match std::str::from_utf8(head) {
        Ok(text) => text,
        Err(error) => std::str::from_utf8(&head[..error.valid_up_to()]).unwrap_or_default(),
    };
    let text = text.trim_start_matches('\u{feff}').trim_start();
    text.starts_with('<') && text.contains("<svg")
}

/// A Markdown image.
pub(super) fn markdown(alt: &str, src: &str) -> String {
    let alt = alt.replace(['[', ']'], "");
    let src = src.trim().replace(' ', "%20");
    format!("![{alt}]({src})")
}

pub(super) fn placeholder(
    theme: &ArtisanTheme,
    icon: Option<AssetId>,
    text: impl Into<SharedString>,
) -> Div {
    let muted = theme.colors.muted_foreground.to_paint();
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(6.0))
        .p(px(12.0))
        .bg(theme.colors.foreground.with_alpha(0.04).to_paint())
        .children(icon.map(|icon| kit::glyph(icon, 20.0, muted)))
        .child(
            kit::muted(theme, text)
                .text_center()
                .max_w_full()
                .truncate(),
        )
}

/// One quiet line saying `src` could not load, in place of its frame.
pub(super) fn unavailable(theme: &ArtisanTheme, src: &str) -> Div {
    let muted = theme.colors.muted_foreground.to_paint();
    div()
        .min_w_0()
        .flex()
        .items_center()
        .gap(px(6.0))
        .child(kit::glyph(AssetId::TABLER_FILE_OFF, 14.0, muted))
        .child(
            kit::muted(theme, format!("Couldn't load {}", src.trim()))
                .min_w_0()
                .truncate(),
        )
}
