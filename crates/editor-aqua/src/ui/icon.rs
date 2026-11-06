use aqua_ui::element::Element;
use editor_assets::{AssetId, Presentation};

use super::{asset::source_asset_glyph, theme::ArtisanTheme};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum IconSize {
    #[default]
    Default,
    Compact,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum IconTint {
    #[default]
    Inherit,
    Muted,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IconStyle {
    id: AssetId,
    pub edge: f32,
    pub presentation: Presentation,
    pub color: Option<aqua_core::GpuiHsla>,
}

impl IconStyle {
    pub fn resolve(theme: ArtisanTheme, id: AssetId, size: IconSize, tint: IconTint) -> Self {
        let presentation = editor_assets::get(id).presentation;
        let color = match (presentation, tint) {
            (Presentation::Tinted, IconTint::Muted) => {
                Some(theme.colors.text_muted.to_reference_paint())
            }
            _ => None,
        };
        let steps = match size {
            IconSize::Default => 4.0,
            IconSize::Compact => 3.5,
        };
        Self {
            id,
            edge: theme.spacing.steps(steps),
            presentation,
            color,
        }
    }

    pub const fn asset_id(&self) -> AssetId {
        self.id
    }
}

pub fn icon(style: IconStyle, inherited_color: aqua_core::GpuiHsla) -> Element {
    source_asset_glyph(style.id, style.edge, style.color.unwrap_or(inherited_color))
}

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
