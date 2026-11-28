use crate::ui::theme::{ArtisanTheme, Elevation};
use aqua_core::GpuiBackground;
use aqua_ui::element::{Box as UiBox, Element};
use aqua_ui::style::{GpuiShadowLayer, Length, Style};

const LINE: f32 = 1.0;

#[must_use]
pub fn sheet(theme: &ArtisanTheme) -> Style {
    Style::new()
        .gpui_background(GpuiBackground::Solid(
            theme.colors.background.to_reference_paint(),
        ))
        .gpui_shadows(
            theme
                .elevation
                .drops(Elevation::Raised)
                .iter()
                .copied()
                .map(GpuiShadowLayer::from)
                .collect(),
        )
}

#[must_use]
pub fn split(theme: &ArtisanTheme) -> Element {
    UiBox::new()
        .style(
            Style::new()
                .width(Length::Px(LINE))
                .gpui_background(GpuiBackground::Solid(
                    theme.colors.line.to_reference_paint(),
                )),
        )
        .build()
}
