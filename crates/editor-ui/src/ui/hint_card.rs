use crate::ui::picker_motion::{DROPDOWN_CLOSE, DROPDOWN_OPEN};
use crate::ui::theme::{ArtisanTheme, Oklch};
use aqua_text::{FontFamily, FontWeight, StyledSpan, TextStyle};
use aqua_ui::element::{Box as UiBox, Element, Text};
use aqua_ui::motion::{Easing, Motion, Presence};
use aqua_ui::style::{Length, Style};

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
    hover_surface(theme, Style::new().width(Length::Px(WIDTH)), content)
}

#[must_use]
pub fn hover_surface(theme: &ArtisanTheme, style: Style, content: Element) -> Element {
    crate::ui::chrome::account::menu_surface(theme, vec![content]).style(style)
}

#[must_use]
pub fn presence() -> Presence {
    Presence::new()
        .enter(Motion::tween(
            DROPDOWN_OPEN.as_secs_f64(),
            Easing::SmoothOut,
        ))
        .exit(Motion::tween(
            DROPDOWN_CLOSE.as_secs_f64(),
            Easing::SmoothOut,
        ))
}
