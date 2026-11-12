use crate::ui::theme::{ArtisanTheme, Elevation};
use aqua_core::{GpuiBackground, Insets};
use aqua_ui::element::{Box as UiBox, Element};
use aqua_ui::style::{GpuiShadowLayer, Length, Style};

const LINE: f32 = 1.0;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Seam {
    Around,
    TopLeft,
}

impl Seam {
    #[must_use]
    pub fn insets(self) -> Insets {
        match self {
            Self::Around => Insets::uniform(LINE),
            Self::TopLeft => Insets {
                top: LINE,
                left: LINE,
                ..Insets::default()
            },
        }
    }

    #[must_use]
    pub fn style(self, theme: &ArtisanTheme) -> Style {
        Style::new().gpui_border(self.insets(), theme.colors.line.to_reference_paint(), false)
    }
}

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
