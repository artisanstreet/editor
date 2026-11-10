use aqua_core::{DisplayItem, Rect, vector::VectorSource};
use aqua_ui::element::{Canvas, Element};
use editor_assets::{AssetId, Presentation};

pub struct SourceSvg {
    pub source: &'static str,
    pub presentation: Presentation,
    pub intrinsic_dimensions: Option<(f32, f32)>,
    pub edge: f32,
    pub tint: aqua_core::GpuiHsla,
}

pub fn source_asset_glyph_factory(
    render: impl Fn(SourceSvg) -> Element + 'static,
) -> impl Fn(AssetId, f32, aqua_core::GpuiHsla) -> Element {
    move |id, edge, tint| {
        let asset = editor_assets::get(id);
        render(SourceSvg {
            source: asset.source,
            presentation: asset.presentation,
            intrinsic_dimensions: asset.intrinsic_dimensions(),
            edge,
            tint,
        })
    }
}

pub fn source_asset_glyph(id: AssetId, edge: f32, tint: aqua_core::GpuiHsla) -> Element {
    let asset = editor_assets::get(id);
    let source: std::sync::Arc<str> = asset.source.into();
    let tint = match asset.presentation {
        Presentation::Tinted => Some(tint),
        Presentation::FullColor => None,
    };
    Canvas::new()
        .width(edge)
        .height(edge)
        .scroll_stable()
        .on_paint(move |bounds| {
            vec![DisplayItem::GpuiSvg {
                bounds,
                source: source.clone(),
                tint,
                opacity: 1.0,
            }]
        })
        .build()
}

pub fn asset_glyph(id: AssetId, edge: f32, tint: aqua_core::Oklch) -> Element {
    let asset = editor_assets::get(id);
    let source = match asset.presentation {
        Presentation::Tinted => VectorSource::tinted_svg(asset.source, tint),
        Presentation::FullColor => VectorSource::svg(asset.source),
    };
    let dimensions = asset.intrinsic_dimensions();
    Canvas::new()
        .width(edge)
        .height(edge)
        .scroll_stable()
        .on_paint(move |bounds| {
            let bounds = contain(bounds, dimensions);
            vec![DisplayItem::Vector {
                bounds,
                source: source.clone(),
                opacity: 1.0,
            }]
        })
        .build()
}

fn contain(bounds: Rect, dimensions: Option<(f32, f32)>) -> Rect {
    let Some((width, height)) = dimensions else {
        return bounds;
    };
    if !bounds.w.is_finite() || !bounds.h.is_finite() || bounds.w <= 0.0 || bounds.h <= 0.0 {
        return bounds;
    }
    let (w, h) = if bounds.w / bounds.h > width / height {
        (bounds.h * width / height, bounds.h)
    } else {
        (bounds.w, bounds.w * height / width)
    };
    Rect {
        x: bounds.x + (bounds.w - w) / 2.0,
        y: bounds.y + (bounds.h - h) / 2.0,
        w,
        h,
    }
}
