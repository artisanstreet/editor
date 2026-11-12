use crate::ui::kit::{Space, Surface, TextRole, Tooltip};
use crate::ui::theme::{ArtisanTheme, Oklch};
use aqua_text::StyledSpan;
use aqua_ui::element::{Box as UiBox, Element, Text};
use aqua_ui::style::{Length, Style};

const WIDTH: f32 = 280.0;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phrase {
    Muted(String),
    Strong(String),
}

#[must_use]
pub fn hint_card(theme: &ArtisanTheme, title: &str, body: &[Phrase]) -> Element {
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
            StyledSpan::new(words, TextRole::Caption.style(theme))
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
        .gap(Space::S4.px())
        .child(
            Text::new(title.to_owned())
                .text_style(TextRole::Title.style(theme))
                .gpui_source_color(theme.colors.text.to_reference_text_color())
                .build(),
        )
        .child(Text::new(sentence).spans(spans).wrap(true).build())
        .build();
    Surface::Tooltip(Tooltip::Card)
        .build(theme, vec![content])
        .style(Style::new().width(Length::Px(WIDTH)))
}
