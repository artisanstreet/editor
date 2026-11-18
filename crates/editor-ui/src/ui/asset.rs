use aqua_core::DisplayItem;
use aqua_ui::element::{Canvas, Element};
use editor_assets::{AssetId, Presentation};

pub(crate) fn source_asset_glyph(id: AssetId, edge: f32, tint: aqua_core::GpuiHsla) -> Element {
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
