//! Loading an image `src` for the image kinds.
//!
//! Sources resolve through the owning Forge, HTTP, or explicit local fixtures.
//! All image bytes use the same bounded decoder and their block's scoped cache.
//! Intrinsic dimensions determine the frame before layout, so image loading
//! cannot change the shared frame's clipping or corner radius.

use std::sync::Arc;

use gpui::{
    App, Asset, Div, ImageSource, IntoElement, ObjectFit, ParentElement as _, RenderImage,
    Resource, SharedString, SharedUri, Styled as _, StyledImage as _, SvgRenderer, Window, div,
    img, px,
};
use serde::Deserialize;

use super::super::kit;
use super::source::{
    Located, MediaFetcher, MediaProvider, MediaRequest, MediaSource, Owned, Payload, locate,
};
use crate::theme::ArtisanTheme;
use editor_assets::AssetId;

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
pub(super) fn load(
    window: &mut Window,
    cx: &mut App,
    owner: &SharedString,
    src: &MediaSource,
    fetcher: Option<&MediaFetcher>,
) -> Loaded {
    let located = locate(src, fetcher, cx);
    let immutable = matches!(&located, Located::Data(_));
    let owner = fetcher.map_or_else(
        || owner.clone(),
        |fetcher| fetcher.asset_owner(owner, immutable),
    );
    let bytes = match located {
        Located::Url(url) => {
            return load_resource(
                window,
                cx,
                &Owned::new(&owner, Resource::Uri(SharedUri::from(url))),
                fetcher,
            );
        }
        #[cfg(test)]
        Located::Path(path) => {
            return load_resource(
                window,
                cx,
                &Owned::new(&owner, Resource::Path(path.into())),
                fetcher,
            );
        }
        Located::Invalid => return Loaded::Failed,
        Located::Data(bytes) => Owned::new(&owner, ImageBytes::Data(bytes)),
        Located::Fetched(request, fetcher) => {
            Owned::new(&owner, ImageBytes::Fetched(request, fetcher))
        }
    };
    let source_bytes = match &bytes.source {
        ImageBytes::Data(payload) => payload.bytes.len(),
        ImageBytes::Fetched(request, _) => request.path.len(),
    } + owner.len();
    let output = match fetcher {
        Some(fetcher) => fetcher.resources.load::<ImageBytesLoader>(
            &fetcher.owner,
            &bytes,
            window,
            cx,
            source_bytes,
            |output| {
                output
                    .as_ref()
                    .map_or(0, |image| image_resident_bytes(image))
            },
            |output, cx| {
                if let Some(image) = output {
                    let image = image.clone();
                    cx.defer(move |cx| {
                        cx.drop_image(image, None);
                        // Other owners may still draw this image in cached scenes.
                        cx.refresh_windows();
                    });
                }
            },
        ),
        None => window.use_asset::<ImageBytesLoader>(&bytes, cx),
    };
    match output {
        None => Loaded::Loading,
        Some(Some(image)) => Loaded::Ready(image),
        Some(None) => Loaded::Failed,
    }
}

fn load_resource(
    window: &mut Window,
    cx: &mut App,
    resource: &Owned<Resource>,
    fetcher: Option<&MediaFetcher>,
) -> Loaded {
    let output = match fetcher {
        Some(fetcher) => fetcher.resources.load::<ResourceLoader>(
            &fetcher.owner,
            resource,
            window,
            cx,
            resource.owner.len() + 256,
            |output| {
                output
                    .as_ref()
                    .map_or(0, |image| image_resident_bytes(image))
            },
            |output, cx| {
                if let Ok(image) = output {
                    let image = image.clone();
                    cx.defer(move |cx| {
                        cx.drop_image(image, None);
                        // Other owners may still draw this image in cached scenes.
                        cx.refresh_windows();
                    });
                }
            },
        ),
        None => window.use_asset::<ResourceLoader>(resource, cx),
    };
    match output {
        None => Loaded::Loading,
        Some(Ok(image)) => Loaded::Ready(image),
        Some(Err(_)) => Loaded::Failed,
    }
}

/// Reads resource bytes and applies the shared bounded image decoder.
enum ResourceLoader {}

impl Asset for ResourceLoader {
    type Source = Owned<Resource>;
    type Output = Result<Arc<RenderImage>, String>;

    fn load(
        Owned { source, .. }: Self::Source,
        cx: &mut App,
    ) -> impl std::future::Future<Output = Self::Output> + Send + 'static {
        let client = cx.http_client();
        let renderer = cx.svg_renderer();
        let assets = cx.asset_source().clone();
        async move {
            let _capacity = super::work_permit().await;
            let bytes = match source {
                Resource::Uri(uri) => {
                    let response = client
                        .get(uri.as_ref(), true)
                        .await
                        .map_err(|error| error.to_string())?;
                    if !response.status.is_success() {
                        return Err(format!("Image request failed: {}", response.status));
                    }
                    response.body
                }
                Resource::Path(path) => {
                    use std::io::Read as _;
                    let file =
                        std::fs::File::open(path.as_ref()).map_err(|error| error.to_string())?;
                    let mut bytes = Vec::new();
                    file.take(MAX_IMAGE_BYTES as u64 + 1)
                        .read_to_end(&mut bytes)
                        .map_err(|error| error.to_string())?;
                    bytes
                }
                Resource::Embedded(path) => assets
                    .load(&path)
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| "Image asset is missing".to_owned())?
                    .to_vec(),
            };
            decode_image_bytes(&bytes, renderer)
        }
    }
}

/// Image bytes that come from neither a URL nor this machine's disk.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum ImageBytes {
    /// A `data:` URL's payload.
    Data(Payload),
    /// A file fetched from where the open thread's files live.
    Fetched(MediaRequest, MediaProvider),
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
            ImageBytes::Fetched(request, fetcher) => Some(fetcher.fetch(request.clone())),
            ImageBytes::Data(_) => None,
        };
        async move {
            let _capacity = super::work_permit().await;
            let bytes = match source {
                ImageBytes::Data(payload) => payload.bytes,
                ImageBytes::Fetched(_, _) => fetch?.await?,
            };
            decode(&bytes, renderer)
        }
    }
}

/// Decodes image bytes by their content, not by any name.
const MAX_IMAGE_BYTES: usize = 32 * 1024 * 1024;
const MAX_DECODED_BYTES: usize = 64 * 1024 * 1024;
const MAX_IMAGE_EDGE: u32 = 8192;
const MAX_ANIMATION_FRAMES: usize = 120;

/// Decode preview bytes on a worker with encoded, decoded and animation budgets.
/// GPUI's SvgRenderer is captured on the UI thread and may be moved into the worker.
pub fn decode_image_bytes(bytes: &[u8], renderer: SvgRenderer) -> Result<Arc<RenderImage>, String> {
    decode(bytes, renderer).ok_or_else(|| "Image is invalid or exceeds preview limits (32 MiB source, 64 MiB decoded, 8192px edges, 120 animation frames)".to_owned())
}

/// Retained CPU pixel bytes, used by an owner's cache budget.
pub fn image_resident_bytes(image: &RenderImage) -> usize {
    (0..image.frame_count())
        .map(|frame| image.as_bytes(frame).map_or(0, <[u8]>::len))
        .sum()
}

fn decode(bytes: &[u8], renderer: SvgRenderer) -> Option<Arc<RenderImage>> {
    use image::{AnimationDecoder as _, ImageDecoder as _};
    use std::io::Cursor;
    if bytes.len() > MAX_IMAGE_BYTES {
        return None;
    }
    if is_svg(bytes) {
        let parsed = renderer.parse_svg(bytes).ok()?;
        let size = parsed.size();
        // GPUI's scale-factor path supersamples both dimensions. Admission
        // must cover the actual pixel allocation, not only the SVG's size.
        let width = (f32::from(size.width) * gpui::SMOOTH_SVG_SCALE_FACTOR).ceil();
        let height = (f32::from(size.height) * gpui::SMOOTH_SVG_SCALE_FACTOR).ceil();
        if !width.is_finite()
            || !height.is_finite()
            || width > MAX_IMAGE_EDGE as f32
            || height > MAX_IMAGE_EDGE as f32
            || width * height * 4.0 > MAX_DECODED_BYTES as f32
        {
            return None;
        }
        return renderer.render_parsed(&parsed, 1.0).ok();
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_EDGE);
    limits.max_image_height = Some(MAX_IMAGE_EDGE);
    limits.max_alloc = Some(MAX_DECODED_BYTES as u64);
    let reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let mut frames = Vec::new();
    if reader.format() == Some(image::ImageFormat::Gif) {
        let mut decoder = image::codecs::gif::GifDecoder::new(Cursor::new(bytes)).ok()?;
        decoder.set_limits(limits).ok()?;
        let mut retained = 0usize;
        for result in decoder.into_frames() {
            let mut frame = result.ok()?;
            retained = retained.checked_add(frame.buffer().as_raw().len())?;
            if retained > MAX_DECODED_BYTES || frames.len() == MAX_ANIMATION_FRAMES {
                return None;
            }
            for pixel in frame.buffer_mut().chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
            frames.push(frame);
        }
    } else {
        let mut reader = reader;
        reader.limits(limits);
        let mut buffer = reader.decode().ok()?.into_rgba8();
        if buffer.as_raw().len() > MAX_DECODED_BYTES {
            return None;
        }
        for pixel in buffer.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        frames.push(image::Frame::new(buffer));
    }
    (!frames.is_empty()).then(|| Arc::new(RenderImage::new(frames)))
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

#[cfg(test)]
mod budget_tests {
    use super::*;
    use crate::asset_seam::CatalogAssetSource;
    fn renderer() -> SvgRenderer {
        SvgRenderer::new(Arc::new(CatalogAssetSource))
    }
    #[test]
    fn svg_dimensions_are_rejected_before_large_raster_allocation() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100000" height="100000"><rect width="100000" height="100000"/></svg>"#;
        assert!(decode_image_bytes(svg, renderer()).is_err());
    }
    #[test]
    fn svg_admission_counts_gpui_supersampling_before_rasterization() {
        let over = br#"<svg xmlns="http://www.w3.org/2000/svg" width="4096" height="4096"/>"#;
        assert!(decode_image_bytes(over, renderer()).is_err());
        let small = br#"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64"/>"#;
        let image = decode_image_bytes(small, renderer()).unwrap();
        assert_eq!(image_resident_bytes(&image), 128 * 128 * 4);
    }
    #[test]
    fn pixel_byte_charge_includes_all_frames() {
        let frames = vec![
            image::Frame::new(image::RgbaImage::new(2, 3)),
            image::Frame::new(image::RgbaImage::new(4, 5)),
        ];
        let rendered = RenderImage::new(frames);
        assert_eq!(image_resident_bytes(&rendered), (2 * 3 + 4 * 5) * 4);
    }
}

#[cfg(test)]
mod repeated_reference_tests {
    use super::*;
    use base64::Engine as _;
    use gpui::{Context, Render, TestAppContext};

    struct Fixture {
        source: MediaSource,
        fetchers: [MediaFetcher; 2],
        showing: [bool; 2],
        images: [Option<Arc<RenderImage>>; 2],
    }
    impl Render for Fixture {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            for index in 0..2 {
                if self.showing[index] {
                    let part: SharedString = if index == 0 {
                        "first-image"
                    } else {
                        "second-image"
                    }
                    .into();
                    if let Loaded::Ready(image) =
                        load(window, cx, &part, &self.source, Some(&self.fetchers[index]))
                    {
                        self.images[index] = Some(image);
                    }
                }
            }
            div()
        }
    }

    #[gpui::test]
    fn repeated_data_image_reuses_decoded_pixels_until_last_item_is_removed(
        cx: &mut TestAppContext,
    ) {
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            2,
            2,
            image::Rgba([12, 34, 56, 255]),
        ))
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
        let source = MediaSource::from(format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(png.into_inner())
        ));
        let provider = MediaFetcher::new("forge/thread", |_| {
            Box::pin(async { panic!("inline pixels must not perform a file fetch") })
        });
        let (view, cx) = cx.add_window_view(|_, _| Fixture {
            source,
            fetchers: [
                provider.clone().owned_by(&[42], 1),
                provider.clone().owned_by(&[43], 2),
            ],
            showing: [true, true],
            images: [None, None],
        });
        cx.run_until_parked();
        let original = view.read_with(cx, |view, _| {
            let first = view.images[0].clone().expect("first image decoded");
            assert!(Arc::ptr_eq(
                &first,
                view.images[1].as_ref().expect("second image decoded")
            ));
            first
        });
        view.update(cx, |view, cx| {
            view.showing[0] = false;
            cx.notify();
        });
        cx.run_until_parked();
        cx.update(|_, cx| provider.retain_resources(&[vec![43]], cx));
        view.update(cx, |view, cx| {
            view.images[1] = None;
            cx.notify();
        });
        cx.run_until_parked();
        view.read_with(cx, |view, _| {
            assert!(Arc::ptr_eq(&original, view.images[1].as_ref().unwrap()))
        });
        view.update(cx, |view, cx| {
            view.showing = [false, false];
            cx.notify();
        });
        cx.run_until_parked();
        cx.update(|_, cx| provider.retain_resources(&[], cx));
        view.update(cx, |view, cx| {
            view.showing[1] = true;
            view.images[1] = None;
            cx.notify();
        });
        cx.run_until_parked();
        view.read_with(cx, |view, _| {
            assert!(
                !Arc::ptr_eq(&original, view.images[1].as_ref().unwrap()),
                "last Item removal evicts the actual decoded allocation"
            )
        });
    }
}
