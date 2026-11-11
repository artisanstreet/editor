use crate::ui::glass::{GlassStrength, glass_card_shadows, glass_layers};
use crate::ui::theme::{ArtisanTheme, Elevation, Oklch, Opacity, RadiusStep, RadiusTokens};
use aqua_core::Insets;
use aqua_text::{FontFamily, FontWeight, StyledSpan, TextStyle};
use aqua_ui::element::{Box as UiBox, Element, Text};
use aqua_ui::style::{GpuiShadowLayer, Style};

const WIDTH: f32 = 280.0;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phrase {
    Muted(String),
    Strong(String),
}

#[must_use]
pub fn hint_card(theme: &ArtisanTheme, title: &str, body: &[Phrase]) -> Element {
    let style = |size, line, weight| TextStyle {
        families: vec![FontFamily::Named(theme.typography.sans.family.to_owned())],
        size_px: size,
        line_height_px: line,
        weight,
        ..TextStyle::default()
    };
    let channels = |color: Oklch| {
        let color = color.to_reference_text_color();
        [
            color.hue_degrees,
            color.saturation,
            color.lightness,
            color.alpha,
        ]
    };
    let spans = body
        .iter()
        .map(|phrase| {
            let (words, color) = match phrase {
                Phrase::Muted(words) => (words, theme.colors.text_muted_glass),
                Phrase::Strong(words) => (words, theme.colors.text),
            };
            StyledSpan::new(
                words,
                style(theme.typography.label_text, 19.0, FontWeight::NORMAL),
            )
            .gpui_source_color(channels(color))
        })
        .collect::<Vec<_>>();
    let sentence = body
        .iter()
        .map(|phrase| match phrase {
            Phrase::Muted(words) | Phrase::Strong(words) => words.as_str(),
        })
        .collect::<String>();
    let content = UiBox::new()
        .layout("column")
        .gap(4.0)
        .padding(16.0)
        .child(
            Text::new(title.to_owned())
                .text_style(style(theme.typography.control_text, 18.0, FontWeight(600)))
                .gpui_source_color(theme.colors.text.to_reference_text_color())
                .build(),
        )
        .child(Text::new(sentence).spans(spans).wrap(true).build())
        .build();
    let radius = RadiusTokens::value(RadiusStep::X2l);
    UiBox::new()
        .width(WIDTH)
        .radius(radius)
        .style(
            Style::new()
                .gpui_border(
                    Insets::uniform(1.0),
                    theme
                        .colors
                        .text
                        .at(Opacity::GlassBorder)
                        .to_reference_paint(),
                    false,
                )
                .gpui_shadows(
                    glass_card_shadows(theme, Elevation::Overlay)
                        .into_iter()
                        .map(GpuiShadowLayer::from)
                        .collect(),
                ),
        )
        .child(glass_layers(theme, GlassStrength::Strong, radius))
        .child(content)
        .build()
}
