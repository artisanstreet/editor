use crate::ui::theme::{ArtisanTheme, Opacity, RadiusStep, RadiusTokens};
use aqua_core::{GpuiBackground, GpuiGradientColorSpace, GpuiGradientStop, Role};
use aqua_ui::element::{Box as UiBox, Element};
use aqua_ui::style::{GpuiShadowLayer, Length, Style};

pub const HEIGHT: f32 = 6.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Normal,
    Warning,
    Danger,
}

impl Tone {
    #[must_use]
    pub fn of(percent: f64) -> Self {
        if percent > 80.0 {
            Self::Danger
        } else if percent > 50.0 {
            Self::Warning
        } else {
            Self::Normal
        }
    }
}

#[must_use]
pub fn bar(theme: &ArtisanTheme, percent: f64, height: f32) -> Element {
    let percent = percent.clamp(0.0, 100.0);
    let fill = match Tone::of(percent) {
        Tone::Normal => GpuiBackground::LinearGradient {
            angle: 90.0,
            color_space: GpuiGradientColorSpace::Srgb,
            stops: [
                GpuiGradientStop {
                    percentage: 0.0,
                    color: theme.colors.action_bottom.to_reference_paint(),
                },
                GpuiGradientStop {
                    percentage: 1.0,
                    color: theme.colors.action_top.to_reference_paint(),
                },
            ],
        },
        Tone::Warning => {
            GpuiBackground::Solid(theme.colors.banner_warning.text.to_reference_paint())
        }
        Tone::Danger => GpuiBackground::Solid(theme.colors.banner_error.text.to_reference_paint()),
    };
    let radius = RadiusTokens::value(RadiusStep::Xs);
    let hairline = |color: crate::ui::theme::Oklch| GpuiShadowLayer {
        dx: 0.0,
        dy: 0.0,
        blur: 0.0,
        spread: 1.0,
        inset: true,
        color: color.to_reference_paint(),
    };
    UiBox::new()
        .width(Length::Pct(100.0))
        .height(height)
        .radius(radius)
        .clip(true)
        .style(
            Style::new()
                .gpui_background(GpuiBackground::Solid(
                    theme.colors.text.at(Opacity::MeterTrack).to_reference_paint(),
                ))
                .gpui_shadows(vec![hairline(theme.colors.text.at(Opacity::GlassBorder))]),
        )
        .child(
            UiBox::new()
                .width(Length::Pct(percent as f32))
                .height(Length::Pct(100.0))
                .radius(radius)
                .style(
                    Style::new()
                        .gpui_background(fill)
                        .gpui_shadows(vec![hairline(theme.colors.text.at(Opacity::GlassBorder))]),
                )
                .build(),
        )
        .build()
        .role(Role::ProgressIndicator)
        .aria_value(format!("{percent:.0}%"))
}
