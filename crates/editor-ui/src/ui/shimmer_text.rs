use aqua_core::{
    NodeId,
    scene::{GpuiTextColorMap, GpuiTextShimmer},
};
use aqua_text::{StyledSpan, TextStyle, overflow::OverflowMode};
use aqua_ui::{
    element::{Box as UiBox, Element, Key, Text},
    style::{LayoutStyle, Length, Style},
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

pub struct ShimmerMotion {
    timing: ShimmerTiming,
    target: aqua_core::GpuiTextColor,
    active: bool,
    identity: Option<u64>,
}

impl ShimmerMotion {
    pub fn new(theme: &ArtisanTheme, timing: ShimmerTiming) -> Self {
        Self {
            timing,
            target: theme.colors.background.to_reference_text_color(),
            active: true,
            identity: None,
        }
    }

    pub fn with_identity(mut self, identity: u64) -> Self {
        self.identity = Some(identity);
        self
    }

    pub fn set_active(&mut self, active: bool) {
        self.active = active;
    }

    pub fn set_theme(&mut self, theme: &ArtisanTheme) {
        self.target = theme.colors.background.to_reference_text_color();
    }

    pub fn sync(&self, app: &mut aqua::UiApp, node: NodeId, reduced_motion: bool) -> bool {
        let Some(bounds) = app.tree().pre_transform_rect(node) else {
            return false;
        };
        let Some(mut paint) = app.tree().paint(node) else {
            return false;
        };
        let style = if self.active && !reduced_motion && bounds.w > 0.0 {
            let raw = node.raw();
            let (duration, delay) = self.timing.durations();
            Style::new().gpui_text_shimmer(GpuiTextShimmer {
                map: GpuiTextColorMap {
                    bounds,
                    phase: 0.0,
                    spread: 50.0,
                    target: self.target,
                },
                key: self
                    .identity
                    .unwrap_or((u64::from(raw.generation) << 32) | u64::from(raw.index)),
                duration,
                delay,
            })
        } else {
            Style::new().clear_text_color_map()
        };
        style.apply_to(&mut LayoutStyle::default(), &mut paint);
        app.tree_mut().set_paint(node, paint)
    }
}

pub struct ShimmerText {
    id: Key,
    content: String,
    theme: ArtisanTheme,
    tone: ShimmerTone,
    truncate: bool,
    spans: Vec<StyledSpan>,
    pub motion: ShimmerMotion,
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
            motion: ShimmerMotion::new(&theme, ShimmerTiming::Status),
        }
    }

    pub fn timing(mut self, timing: ShimmerTiming) -> Self {
        self.motion.timing = timing;
        self
    }

    pub fn active(mut self, active: bool) -> Self {
        self.motion.set_active(active);
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
        UiBox::new()
            .style(
                Style::new()
                    .min_width(Length::Px(0.0))
                    .gpui_source_text_color(color),
            )
            .child(text.build())
            .build()
            .with_key(self.id.clone())
    }
}

pub struct ShimmerLine {
    id: Key,
    pub motion: ShimmerMotion,
}

impl ShimmerLine {
    pub fn new(id: impl Into<Key>, theme: &ArtisanTheme) -> Self {
        Self {
            id: id.into(),
            motion: ShimmerMotion::new(theme, ShimmerTiming::Status),
        }
    }

    pub fn timing(mut self, timing: ShimmerTiming) -> Self {
        self.motion.timing = timing;
        self
    }

    pub fn active(mut self, active: bool) -> Self {
        self.motion.set_active(active);
        self
    }

    pub fn build(&self, child: Element) -> Element {
        child
    }

    pub fn id(&self) -> &Key {
        &self.id
    }
}
