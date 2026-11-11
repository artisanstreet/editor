use aqua_core::scene::TextShimmer;
use aqua_text::{StyledSpan, TextStyle, overflow::OverflowMode};
use aqua_ui::{
    element::{Box as UiBox, Element, Key, Text},
    style::{Length, Style},
    tree::hash_key,
};

use super::theme::ArtisanTheme;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ShimmerTone {
    #[default]
    Foreground,
    Muted,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ShimmerTiming {
    #[default]
    Status,
    Progress,
}

impl ShimmerTiming {
    fn durations(self) -> (std::time::Duration, std::time::Duration) {
        match self {
            Self::Status => (
                std::time::Duration::from_secs(3),
                std::time::Duration::from_millis(1_500),
            ),
            Self::Progress => (std::time::Duration::from_secs(2), std::time::Duration::ZERO),
        }
    }
}

pub fn sweep(theme: &ArtisanTheme, timing: ShimmerTiming, key: u64) -> TextShimmer {
    let (duration, delay) = timing.durations();
    TextShimmer {
        key,
        spread: 50.0,
        target: theme.colors.background.to_reference_text_color(),
        duration,
        delay,
    }
}

pub struct ShimmerText {
    id: Key,
    content: String,
    theme: ArtisanTheme,
    tone: ShimmerTone,
    truncate: bool,
    spans: Vec<StyledSpan>,
    timing: ShimmerTiming,
    active: bool,
    reduced_motion: bool,
}

impl ShimmerText {
    pub fn new(id: impl Into<Key>, content: impl Into<String>, theme: ArtisanTheme) -> Self {
        Self {
            id: id.into(),
            content: content.into(),
            theme,
            tone: ShimmerTone::Foreground,
            truncate: false,
            spans: Vec::new(),
            timing: ShimmerTiming::Status,
            active: true,
            reduced_motion: false,
        }
    }

    pub fn timing(mut self, timing: ShimmerTiming) -> Self {
        self.timing = timing;
        self
    }

    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    pub fn reduced_motion(mut self, reduced_motion: bool) -> Self {
        self.reduced_motion = reduced_motion;
        self
    }

    pub fn tone(mut self, tone: ShimmerTone) -> Self {
        self.tone = tone;
        self
    }
    pub fn truncate(mut self) -> Self {
        self.truncate = true;
        self
    }
    pub fn spans(mut self, spans: Vec<StyledSpan>) -> Self {
        self.spans = spans;
        self
    }

    pub fn build(&self, typography: TextStyle) -> Element {
        let color = match self.tone {
            ShimmerTone::Foreground => self.theme.colors.text,
            ShimmerTone::Muted => self.theme.colors.text_muted,
        }
        .to_reference_text_color();
        let mut text = Text::new(self.content.clone())
            .text_style(typography)
            .gpui_source_color(color);
        if !self.spans.is_empty() {
            text = text.spans(self.spans.clone());
        }
        if self.truncate {
            text = text.overflow(OverflowMode::EndEllipsis);
        }
        let style = Style::new()
            .min_width(Length::Px(0.0))
            .gpui_source_text_color(color);
        let style = if self.active && !self.reduced_motion {
            style.text_shimmer(sweep(
                &self.theme,
                self.timing,
                hash_key(self.id.clone()),
            ))
        } else {
            style
        };
        UiBox::new()
            .style(style)
            .child(text.build())
            .build()
            .with_key(self.id.clone())
    }
}
