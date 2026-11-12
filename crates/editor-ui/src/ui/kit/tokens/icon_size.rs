use crate::ui::asset::source_asset_glyph;
use aqua_core::GpuiHsla;
use aqua_ui::element::Element;
use editor_assets::AssetId;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum IconSize {
    Sm,
    Md,
    Lg,
}

impl IconSize {
    #[must_use]
    pub const fn px(self) -> f32 {
        match self {
            Self::Sm => 12.0,
            Self::Md => 16.0,
            Self::Lg => 20.0,
        }
    }

    #[must_use]
    pub fn glyph(self, icon: AssetId, tint: GpuiHsla) -> Element {
        source_asset_glyph(icon, self.px(), tint)
    }
}
