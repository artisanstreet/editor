//! Size-aware raster presentation for cataloged full-colour SVG artwork.
//!
//! GPUI's ordinary `Img` asset loader intentionally rasterizes SVG resources
//! at their intrinsic size. That is a good general image-loader default, but
//! it makes small authored-colour glyphs rely on a later bilinear reduction.
//! This element keeps the normal GPUI layout/interactivity/opacity/mask path
//! and only changes the SVG raster target: the actual laid-out glyph size in
//! device pixels, with the same two-times smoothing pass used by GPUI's
//! monochrome SVG route.

#![forbid(unsafe_code)]

use std::{collections::VecDeque, sync::Arc};

use artisan_assets::AssetId;
use gpui::{
    App, Bounds, DefiniteLength, DevicePixels, Element, ElementId, Global, GlobalElementId, Hitbox,
    InspectorElementId, Interactivity, IntoElement, LayoutId, Length, Pixels, RenderImage, Size,
    StyleRefinement, Styled, SvgRenderer, SvgSize, Window, point, px, size,
};

/// The supersampling factor used before GPUI composites the glyph into its
/// logical bounds. This matches the pinned renderer's tinted SVG route.
const SUPERSAMPLE_FACTOR: f32 = 2.0;
/// A catalog glyph is never expected to need a texture larger than this.
/// Capping here also keeps a malformed layout from allocating an unbounded
/// raster before the renderer's own safety cap is reached.
const MAX_RASTER_DIMENSION: u32 = 1024;
/// Area cap for one cached raster, in physical pixels.
const MAX_RASTER_PIXELS: u64 = 1_048_576;
/// Bounded parsed-document retention per GPUI application.
const PARSED_CACHE_CAPACITY: usize = 64;
/// Bounded raster retention per GPUI application.
const RASTER_CACHE_CAPACITY: usize = 128;
const RASTER_CACHE_BYTES: usize = 8 * 1024 * 1024;

/// A full-colour SVG element with the regular GPUI style and hitbox surface.
pub(crate) struct FullColorSvg {
    interactivity: Interactivity,
    asset_id: AssetId,
}

impl FullColorSvg {
    pub(crate) fn new(asset_id: AssetId) -> Self {
        Self {
            interactivity: Interactivity::new(),
            asset_id,
        }
    }

    fn rasterized(
        &self,
        bounds: Size<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<CachedSvgImage> {
        let asset = artisan_assets::lookup(self.asset_id.as_str()).ok()?;
        let renderer = cx.svg_renderer();
        let cache = cx.default_global::<FullColorSvgCache>();
        let result = cache.render_for(
            self.asset_id,
            bounds,
            window.scale_factor(),
            &renderer,
            asset.source.as_bytes(),
        );
        let evicted = std::mem::take(&mut cache.evicted);
        for image in evicted {
            cx.drop_image(image, Some(window));
        }
        result
    }
}

pub(crate) struct FullColorSvgPrepaintState {
    hitbox: Option<Hitbox>,
    image: Option<CachedSvgImage>,
}

impl Element for FullColorSvg {
    type RequestLayoutState = ();
    type PrepaintState = FullColorSvgPrepaintState;

    fn id(&self) -> Option<ElementId> {
        self.interactivity.element_id.clone()
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        self.interactivity.source_location()
    }

    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let renderer = cx.svg_renderer();
        let intrinsic = cx
            .default_global::<FullColorSvgCache>()
            .parsed_for(
                self.asset_id,
                &renderer,
                artisan_assets::get(self.asset_id).source.as_bytes(),
            )
            .map(|(_, size)| size.map(|v| px(v.0 as f32 / SUPERSAMPLE_FACTOR)));
        let layout_id = self.interactivity.request_layout(
            global_id,
            inspector_id,
            window,
            cx,
            |mut style, window, cx| {
                if let Some(intrinsic) = intrinsic {
                    style
                        .aspect_ratio
                        .get_or_insert(intrinsic.width / intrinsic.height);
                    if let Length::Auto = style.size.width {
                        style.size.width = Length::Definite(match style.size.height {
                            Length::Definite(DefiniteLength::Absolute(length)) => {
                                px(f32::from(length.to_pixels(window.rem_size()))
                                    * (intrinsic.width / intrinsic.height))
                                .into()
                            }
                            _ => intrinsic.width.into(),
                        });
                    }
                    if let Length::Auto = style.size.height {
                        style.size.height = Length::Definite(match style.size.width {
                            Length::Definite(DefiniteLength::Absolute(length)) => {
                                px(f32::from(length.to_pixels(window.rem_size()))
                                    * (intrinsic.height / intrinsic.width))
                                .into()
                            }
                            _ => intrinsic.height.into(),
                        });
                    }
                }
                window.request_layout(style, None, cx)
            },
        );
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let hitbox = self.interactivity.prepaint(
            global_id,
            inspector_id,
            bounds,
            bounds.size,
            window,
            cx,
            |_, _, hitbox, _, _| hitbox,
        );
        let image = self.rasterized(bounds.size, window, cx);
        FullColorSvgPrepaintState { hitbox, image }
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.interactivity.paint(
            global_id,
            inspector_id,
            bounds,
            prepaint.hitbox.as_ref(),
            window,
            cx,
            |style, window, _cx| {
                let Some(image) = prepaint.image.as_ref() else {
                    return;
                };
                let corner_radii = style.corner_radii.to_pixels(window.rem_size());
                let _ = window.paint_image(
                    bounds,
                    Bounds {
                        origin: bounds.origin
                            + point(
                                (bounds.size.width - image.display_size.width) / 2.0,
                                (bounds.size.height - image.display_size.height) / 2.0,
                            ),
                        size: image.display_size,
                    },
                    corner_radii,
                    Arc::clone(&image.image),
                    0,
                    false,
                );
            },
        );
    }
}

impl IntoElement for FullColorSvg {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Styled for FullColorSvg {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.interactivity.base_style
    }
}

/// The raster returned by the bounded cache plus the logical size at which
/// it should be painted. The latter is recomputed for each layout, so one
/// physical raster can safely be reused by windows whose logical scales
/// differ but resolve to the same device dimensions.
struct CachedSvgImage {
    image: Arc<RenderImage>,
    display_size: Size<Pixels>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct RasterKey {
    asset_id: AssetId,
    width: u32,
    height: u32,
}

struct ParsedSvgEntry {
    asset_id: AssetId,
    parsed: Arc<gpui::ParsedSvg>,
    intrinsic_size: Size<DevicePixels>,
}

struct RasterEntry {
    key: RasterKey,
    image: Arc<RenderImage>,
}

/// App-owned, bounded cache for parsed catalog documents and size-specific
/// full-colour rasters. `VecDeque` order is MRU at the front, so resizes can
/// evict old dimensions without accumulating an unbounded per-DPI history.
#[derive(Default)]
pub(crate) struct FullColorSvgCache {
    parsed: VecDeque<ParsedSvgEntry>,
    rasters: VecDeque<RasterEntry>,
    raster_bytes: usize,
    evicted: Vec<Arc<RenderImage>>,
}

impl Global for FullColorSvgCache {}

impl FullColorSvgCache {
    fn render_for(
        &mut self,
        asset_id: AssetId,
        bounds: Size<Pixels>,
        scale_factor: f32,
        renderer: &SvgRenderer,
        bytes: &[u8],
    ) -> Option<CachedSvgImage> {
        if !f32::from(bounds.width).is_finite()
            || !f32::from(bounds.height).is_finite()
            || bounds.width <= px(0.0)
            || bounds.height <= px(0.0)
            || !scale_factor.is_finite()
            || scale_factor <= 0.0
        {
            return None;
        }
        let (parsed, intrinsic_size) = self.parsed_for(asset_id, renderer, bytes)?;
        let display_size = contain_size(bounds, intrinsic_size);
        let target = raster_target(display_size, scale_factor);
        if target.width.0 <= 0 || target.height.0 <= 0 {
            return None;
        }

        let key = RasterKey {
            asset_id,
            width: u32::from(target.width),
            height: u32::from(target.height),
        };
        let image = if let Some(image) = self.cached_raster(key) {
            image
        } else {
            let image = renderer
                .render_parsed(parsed.as_ref(), SvgSize::ExactSize(target))
                .ok()?;
            self.insert_raster(key, Arc::clone(&image));
            image
        };

        Some(CachedSvgImage {
            image,
            display_size,
        })
    }

    fn parsed_for(
        &mut self,
        asset_id: AssetId,
        renderer: &SvgRenderer,
        bytes: &[u8],
    ) -> Option<(Arc<gpui::ParsedSvg>, Size<DevicePixels>)> {
        if let Some(index) = self
            .parsed
            .iter()
            .position(|entry| entry.asset_id == asset_id)
        {
            let entry = self.parsed.remove(index)?;
            let result = (Arc::clone(&entry.parsed), entry.intrinsic_size);
            self.parsed.push_front(entry);
            return Some(result);
        }

        let parsed = Arc::new(renderer.parse_svg(bytes).ok()?);
        // Rendering once at the document's natural scale supplies a stable
        // aspect ratio without introducing a second XML parser. The resulting
        // image is not retained; all visible rasters are keyed by layout size.
        let intrinsic_size = renderer
            .render_parsed(parsed.as_ref(), SvgSize::ScaleFactor(1.0))
            .ok()?
            .size(0);
        if intrinsic_size.width.0 <= 0 || intrinsic_size.height.0 <= 0 {
            return None;
        }
        self.parsed.push_front(ParsedSvgEntry {
            asset_id,
            parsed: Arc::clone(&parsed),
            intrinsic_size,
        });
        while self.parsed.len() > PARSED_CACHE_CAPACITY {
            self.parsed.pop_back();
        }
        Some((parsed, intrinsic_size))
    }

    fn cached_raster(&mut self, key: RasterKey) -> Option<Arc<RenderImage>> {
        let index = self.rasters.iter().position(|entry| entry.key == key)?;
        let entry = self.rasters.remove(index)?;
        let image = Arc::clone(&entry.image);
        self.rasters.push_front(entry);
        Some(image)
    }

    fn insert_raster(&mut self, key: RasterKey, image: Arc<RenderImage>) {
        self.raster_bytes += image.as_bytes(0).map_or(0, |bytes| bytes.len());
        self.rasters.push_front(RasterEntry { key, image });
        while self.rasters.len() > RASTER_CACHE_CAPACITY || self.raster_bytes > RASTER_CACHE_BYTES {
            if let Some(entry) = self.rasters.pop_back() {
                self.raster_bytes -= entry.image.as_bytes(0).map_or(0, |bytes| bytes.len());
                self.evicted.push(entry.image);
            }
        }
    }
}

fn contain_size(bounds: Size<Pixels>, intrinsic: Size<DevicePixels>) -> Size<Pixels> {
    let width = f32::from(bounds.width).max(0.0);
    let height = f32::from(bounds.height).max(0.0);
    let intrinsic_width = u32::from(intrinsic.width) as f32;
    let intrinsic_height = u32::from(intrinsic.height) as f32;
    if width <= 0.0 || height <= 0.0 || intrinsic_width <= 0.0 || intrinsic_height <= 0.0 {
        return size(px(0.0), px(0.0));
    }

    let intrinsic_ratio = intrinsic_width / intrinsic_height;
    let bounds_ratio = width / height;
    if bounds_ratio > intrinsic_ratio {
        size(px(height * intrinsic_ratio), px(height))
    } else {
        size(px(width), px(width / intrinsic_ratio))
    }
}

fn raster_target(display_size: Size<Pixels>, scale_factor: f32) -> Size<DevicePixels> {
    let physical_scale = (scale_factor.max(0.01) * SUPERSAMPLE_FACTOR).max(1.0);
    let width = (f32::from(display_size.width) * physical_scale)
        .ceil()
        .max(1.0) as u32;
    let height = (f32::from(display_size.height) * physical_scale)
        .ceil()
        .max(1.0) as u32;
    capped_size(width, height)
}

fn capped_size(width: u32, height: u32) -> Size<DevicePixels> {
    let width = width.max(1);
    let height = height.max(1);
    let dimension_scale = (MAX_RASTER_DIMENSION as f32 / width as f32)
        .min(MAX_RASTER_DIMENSION as f32 / height as f32)
        .min(1.0);
    let area = (width as f32) * (height as f32);
    let area_scale = (MAX_RASTER_PIXELS as f32 / area).sqrt().min(1.0);
    let scale = dimension_scale.min(area_scale);
    let capped_width = ((width as f32 * scale).round() as u32).max(1);
    let capped_height = ((height as f32 * scale).round() as u32).max(1);
    Size::new(
        DevicePixels(capped_width as i32),
        DevicePixels(capped_height as i32),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset_seam::CatalogAssetSource;

    fn renderer() -> SvgRenderer {
        SvgRenderer::new(Arc::new(CatalogAssetSource))
    }

    fn has_authored_colour_and_alpha(image: &RenderImage) -> bool {
        image
            .as_bytes(0)
            .into_iter()
            .flat_map(|bytes| bytes.chunks_exact(4))
            .any(|pixel| {
                pixel[3] > 0 && pixel[3] < 255 && (pixel[0] != pixel[1] || pixel[1] != pixel[2])
            })
    }

    #[test]
    fn real_full_colour_catalog_marks_keep_colour_and_translucent_edges() {
        let renderer = renderer();
        let mut cache = FullColorSvgCache::default();
        for asset_id in [AssetId::SVGL_CLAUDE_AI, AssetId::SVGL_GITLAB] {
            let rendered = cache
                .render_for(
                    asset_id,
                    size(px(20.0), px(20.0)),
                    1.0,
                    &renderer,
                    artisan_assets::get(asset_id).source.as_bytes(),
                )
                .expect("catalog full-colour SVG should render");
            assert!(
                has_authored_colour_and_alpha(&rendered.image),
                "{asset_id} must retain authored chroma and antialiased alpha"
            );
            if asset_id == AssetId::SVGL_CLAUDE_AI {
                assert!(
                    rendered
                        .image
                        .as_bytes(0)
                        .unwrap()
                        .chunks_exact(4)
                        .any(|pixel| pixel == [0x57, 0x77, 0xd9, 0xff]),
                    "Claude's authored clay colour must survive BGRA conversion"
                );
            }
        }
    }

    #[test]
    fn fractional_scale_changes_physical_target_dimensions() {
        let logical = size(px(16.0), px(20.0));
        let one_x = raster_target(logical, 1.0);
        let fractional = raster_target(logical, 1.25);
        assert_ne!(one_x, fractional);
        assert_eq!(one_x.width, DevicePixels(32));
        assert_eq!(one_x.height, DevicePixels(40));
        assert_eq!(fractional.width, DevicePixels(40));
        assert_eq!(fractional.height, DevicePixels(50));
    }

    #[test]
    fn same_asset_and_physical_size_reuses_cached_raster_and_eviction_is_bounded() {
        let renderer = renderer();
        let mut cache = FullColorSvgCache::default();
        let asset_id = AssetId::SVGL_CLAUDE_AI;
        let bytes = artisan_assets::get(asset_id).source.as_bytes();
        let first = cache
            .render_for(asset_id, size(px(20.0), px(20.0)), 1.0, &renderer, bytes)
            .expect("first raster");
        let second = cache
            .render_for(asset_id, size(px(20.0), px(20.0)), 1.0, &renderer, bytes)
            .expect("cached raster");
        assert!(Arc::ptr_eq(&first.image, &second.image));
        let equivalent = cache
            .render_for(asset_id, size(px(10.0), px(10.0)), 2.0, &renderer, bytes)
            .unwrap();
        assert!(Arc::ptr_eq(&first.image, &equivalent.image));

        for edge in 1..=(RASTER_CACHE_CAPACITY as u32 + 4) {
            let _ = cache.render_for(
                asset_id,
                size(px(edge as f32), px(edge as f32)),
                1.0,
                &renderer,
                bytes,
            );
        }
        assert!(cache.rasters.len() <= RASTER_CACHE_CAPACITY);
        assert!(cache.raster_bytes <= RASTER_CACHE_BYTES);
        assert!(!cache.evicted.is_empty());
        assert_eq!(cache.parsed.len(), 1);
    }

    #[test]
    fn fitted_target_preserves_intrinsic_aspect_ratio_without_stretching() {
        let fitted = contain_size(
            size(px(80.0), px(40.0)),
            Size::new(DevicePixels(200), DevicePixels(100)),
        );
        assert_eq!(fitted, size(px(80.0), px(40.0)));

        let letterboxed = contain_size(
            size(px(40.0), px(80.0)),
            Size::new(DevicePixels(200), DevicePixels(100)),
        );
        assert_eq!(letterboxed, size(px(40.0), px(20.0)));
    }

    #[gpui::test]
    fn real_element_preserves_auto_height_reuses_rasters_and_releases_evicted_gpu_images(
        cx: &mut gpui::TestAppContext,
    ) {
        let cx = cx.add_empty_window();
        let draw = |cx: &mut gpui::VisualTestContext, edge: f32| {
            cx.draw(
                point(px(0.0), px(0.0)),
                size(px(400.0), px(400.0)),
                move |_, _| FullColorSvg::new(AssetId::SVGL_CLAUDE_AI).w(px(edge)),
            );
        };
        draw(cx, 20.0);
        let original = cx.update(|window, app| {
            let cache = app.global::<FullColorSvgCache>();
            let raster = cache
                .rasters
                .front()
                .expect("width-only image must retain its auto height");
            assert!(raster.key.height > 0);
            assert!(window.has_image_atlas_entry(&raster.image));
            Arc::clone(&raster.image)
        });
        draw(cx, 20.0);
        cx.update(|_, app| {
            assert!(Arc::ptr_eq(
                &original,
                &app.global::<FullColorSvgCache>()
                    .rasters
                    .front()
                    .unwrap()
                    .image
            ))
        });
        for edge in 30..170 {
            draw(cx, edge as f32);
        }
        cx.update(|window, app| {
            let cache = app.global::<FullColorSvgCache>();
            assert!(cache.rasters.len() <= RASTER_CACHE_CAPACITY);
            assert!(cache.raster_bytes <= RASTER_CACHE_BYTES);
            assert!(cache.evicted.is_empty());
            assert!(
                !window.has_image_atlas_entry(&original),
                "eviction must release GPU atlas storage"
            );
        });
    }
}
