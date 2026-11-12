use crate::ui::theme::{ArtisanTheme, ProseTypography};
use aqua_text::{FontFamily, FontWeight, TextStyle};
use aqua_ui::element::{Text, TextSource};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Face {
    Sans,
    Mono,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextSpec {
    pub size: f32,
    pub line: f32,
    pub weight: FontWeight,
    pub face: Face,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TextRole {
    Caption,
    Label,
    LabelStrong,
    Title,
    Prose,
    Code,
    Heading,
}

const fn sans(size: f32, line: f32, weight: FontWeight) -> TextSpec {
    TextSpec {
        size,
        line,
        weight,
        face: Face::Sans,
    }
}

impl TextRole {
    pub const ALL: [Self; 7] = [
        Self::Caption,
        Self::Label,
        Self::LabelStrong,
        Self::Title,
        Self::Prose,
        Self::Code,
        Self::Heading,
    ];

    #[must_use]
    pub const fn spec(self) -> TextSpec {
        match self {
            Self::Caption => sans(12.0, 16.0, FontWeight::NORMAL),
            Self::Label => sans(14.0, 20.0, FontWeight::NORMAL),
            Self::LabelStrong => sans(14.0, 20.0, FontWeight::MEDIUM),
            Self::Title => sans(14.0, 20.0, FontWeight(600)),
            Self::Prose => sans(
                ProseTypography::BODY_SIZE_PX,
                ProseTypography::BODY_LINE_PX,
                ProseTypography::BODY_WEIGHT,
            ),
            Self::Code => TextSpec {
                size: ProseTypography::CODE_SIZE_PX,
                line: ProseTypography::CODE_LINE_PX,
                weight: FontWeight::NORMAL,
                face: Face::Mono,
            },
            Self::Heading => sans(20.0, 28.0, FontWeight::MEDIUM),
        }
    }

    #[must_use]
    pub fn style(self, theme: &ArtisanTheme) -> TextStyle {
        let spec = self.spec();
        let family = match spec.face {
            Face::Sans => theme.typography.sans.family,
            Face::Mono => theme.typography.mono.family,
        };
        TextStyle {
            families: vec![FontFamily::Named(family.to_owned())],
            size_px: spec.size,
            line_height_px: spec.line,
            weight: spec.weight,
            ..TextStyle::default()
        }
    }

    #[must_use]
    pub fn text(self, theme: &ArtisanTheme, words: impl TextSource) -> Text {
        Text::new(words)
            .text_style(self.style(theme))
            .gpui_source_color(theme.colors.text.to_reference_text_color())
    }
}
