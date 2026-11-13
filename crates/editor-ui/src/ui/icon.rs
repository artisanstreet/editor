use aqua_ui::element::Element;
use editor_assets::AssetId;

pub fn disclosure_chevron(fraction: f32, edge: f32, color: aqua_core::GpuiHsla) -> Element {
    let source: std::sync::Arc<str> = editor_assets::get(AssetId::TABLER_CHEVRON_RIGHT)
        .source
        .into();
    aqua_ui::element::Canvas::new()
        .width(edge)
        .height(edge)
        .style(aqua_ui::style::Style::new().flex_shrink(0.0))
        .scroll_stable()
        .on_paint(move |bounds| {
            vec![aqua_core::DisplayItem::GpuiTransformedSvg {
                bounds,
                source: source.clone(),
                tint: color,
                opacity: 1.0,
                transform: aqua_core::scene::GpuiSvgTransform {
                    rotation_radians: fraction * std::f32::consts::FRAC_PI_2,
                    ..Default::default()
                },
            }]
        })
        .build()
}

pub fn file_icon(path: &std::path::Path) -> (AssetId, AssetId) {
    (
        editor_assets::file_icons::file(path, false),
        editor_assets::file_icons::file(path, true),
    )
}
