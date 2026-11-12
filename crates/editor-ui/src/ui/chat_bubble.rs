use aqua_core::{CornerRadii, GpuiBackground, GpuiGradientColorSpace, GpuiGradientStop, Insets};
use aqua_ui::{
    element::{Box as UiBox, Element},
    style::{Length, Style},
};

use super::{
    glass::card_shadows,
    kit::{Surface, TextRole},
    theme::{ArtisanTheme, Oklch, Opacity},
};

pub const SURFACE: Surface = Surface::Bubble;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegmentPosition {
    Single,
    Head,
    Body,
    Tail,
}

impl SegmentPosition {
    pub const fn for_index(index: usize, count: usize) -> Self {
        match (index == 0, index + 1 >= count) {
            (true, true) => Self::Single,
            (true, false) => Self::Head,
            (false, true) => Self::Tail,
            (false, false) => Self::Body,
        }
    }

    const fn first(self) -> bool {
        matches!(self, Self::Single | Self::Head)
    }

    const fn last(self) -> bool {
        matches!(self, Self::Single | Self::Tail)
    }
}

pub struct ChatBubbleSegment {
    theme: ArtisanTheme,
    position: SegmentPosition,
    content: Element,
    answer: bool,
    tint: Option<Oklch>,
    selector: Option<String>,
}

impl ChatBubbleSegment {
    pub const MAX_WIDTH: f32 = 576.0;

    pub fn new(theme: ArtisanTheme, position: SegmentPosition, content: Element) -> Self {
        Self {
            theme,
            position,
            content,
            answer: false,
            tint: None,
            selector: None,
        }
    }

    pub const fn answer(mut self) -> Self {
        self.answer = true;
        self
    }

    pub const fn tinted(mut self, tint: Oklch) -> Self {
        self.tint = Some(tint);
        self
    }

    pub fn debug_selector(mut self, selector: impl Into<String>) -> Self {
        self.selector = Some(selector.into());
        self
    }

    pub fn build(self) -> Element {
        let first = self.position.first();
        let last = self.position.last();
        let foot = if self.theme.colors.mode.is_dark() {
            4
        } else {
            3
        };
        let top = self
            .theme
            .colors
            .gray
            .step(if first { foot + 1 } else { foot });
        let bottom = self.theme.colors.gray.step(foot);
        let radius = SURFACE.radius_px();
        let pad = SURFACE.padding_px();
        let corners = CornerRadii {
            top_left: if first { radius } else { 0.0 },
            top_right: if first { radius } else { 0.0 },
            bottom_left: if last { radius } else { 0.0 },
            bottom_right: if last { radius } else { 0.0 },
        };
        let mut style = Style::new()
            .max_width(Length::Px(Self::MAX_WIDTH))
            .corners(corners)
            .padding_insets(Insets {
                left: pad,
                right: pad,
                top: if first { pad } else { 0.0 },
                bottom: if last { pad } else { 0.0 },
            })
            .gpui_background(GpuiBackground::LinearGradient {
                angle: 180.0,
                color_space: GpuiGradientColorSpace::Oklab,
                stops: [
                    GpuiGradientStop {
                        percentage: 0.0,
                        color: top.to_reference_paint(),
                    },
                    GpuiGradientStop {
                        percentage: 1.0,
                        color: bottom.to_reference_paint(),
                    },
                ],
            })
            .text_style(TextRole::Prose.style(&self.theme))
            .gpui_source_text_color(self.theme.colors.text.to_reference_text_color())
            .text_wrap(true);
        if self.position == SegmentPosition::Single {
            style = style.gpui_shadows(
                card_shadows(&self.theme)
                    .into_iter()
                    .map(aqua_ui::style::GpuiShadowLayer::from)
                    .collect(),
            );
        } else {
            style = style.width(Length::Pct(100.0));
        }
        if let Some(tint) = self.tint {
            style = style.gpui_border(
                Insets {
                    left: 1.0,
                    right: 1.0,
                    top: if first { 1.0 } else { 0.0 },
                    bottom: if last { 1.0 } else { 0.0 },
                },
                tint.at(Opacity::EngineEdge).to_reference_paint(),
                false,
            );
        }
        if self.answer {
            style = style
                .width(Length::Pct(100.0))
                .min_width(Length::Px(0.0))
                .gpui_border(
                    Insets::uniform(1.0),
                    self.theme.colors.gray.step(8).to_reference_paint(),
                    true,
                );
        }
        let mut frame = UiBox::new().style(style);
        if let Some(tint) = self.tint {
            frame = frame.child(
                UiBox::new()
                    .style(
                        Style::new()
                            .absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                            .corners(corners)
                            .gpui_background(GpuiBackground::Solid(
                                tint.at(Opacity::EngineLean).to_reference_paint(),
                            )),
                    )
                    .build(),
            );
        }
        let element = frame.child(self.content).build();
        if let Some(selector) = self.selector {
            element.debug_selector(selector)
        } else {
            element
        }
    }
}
