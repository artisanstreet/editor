use super::Surface;
use crate::ui::chrome::card::{Edge, Surfaces};
use crate::ui::kit::divider::Divider;
use crate::ui::kit::hover::Hover;
use crate::ui::kit::row::Row;
use crate::ui::theme::{ArtisanTheme, Oklch};
use aqua_core::geometry::CornerRadii;
use aqua_core::{GpuiBackground, GpuiGradientColorSpace, GpuiGradientStop, Insets};
use aqua_ui::element::{Box as UiBox, Element};
use aqua_ui::style::{Length, Style};

const CARD: Surface = Surface::Card;
const RIM: f32 = 1.0;
const HIGHLIGHT: f32 = 2.0;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Band {
    Only,
    First,
    Middle,
    Last,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sides {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl Band {
    #[must_use]
    pub const fn of(first: bool, last: bool) -> Self {
        match (first, last) {
            (true, true) => Self::Only,
            (true, false) => Self::First,
            (false, true) => Self::Last,
            (false, false) => Self::Middle,
        }
    }

    #[must_use]
    pub const fn opens(self) -> bool {
        matches!(self, Self::Only | Self::First)
    }

    #[must_use]
    pub const fn closes(self) -> bool {
        matches!(self, Self::Only | Self::Last)
    }

    #[must_use]
    pub fn inset(self, depth: f32) -> Sides {
        let end = |closed: bool| if closed { depth } else { 0.0 };
        Sides {
            top: end(self.opens()),
            right: depth,
            bottom: end(self.closes()),
            left: depth,
        }
    }

    #[must_use]
    pub fn padding(self) -> Sides {
        self.inset(CARD.padding_px())
    }

    #[must_use]
    pub fn corners(self, radius: f32) -> CornerRadii {
        let top = if self.opens() { radius } else { 0.0 };
        let bottom = if self.closes() { radius } else { 0.0 };
        CornerRadii {
            top_left: top,
            top_right: top,
            bottom_left: bottom,
            bottom_right: bottom,
        }
    }

    fn rim(self, edge: &Edge) -> GpuiBackground {
        let middle = edge.rim_top.animate_to(edge.rim_bottom, 0.5);
        let (top, bottom) = match self {
            Self::Only => (edge.rim_top, edge.rim_bottom),
            Self::First => (edge.rim_top, middle),
            Self::Middle => (middle, middle),
            Self::Last => (middle, edge.rim_bottom),
        };
        vertical(top, bottom)
    }

    fn layer(self, depth: f32, background: GpuiBackground) -> Element {
        let at = self.inset(depth);
        let radius = CARD.radius_px() - depth;
        UiBox::new()
            .style(
                Style::new()
                    .absolute_at(Some(at.left), Some(at.top), Some(at.right), Some(at.bottom))
                    .corners(self.corners(radius))
                    .gpui_background(background),
            )
            .build()
            .exclude_from_hit_testing()
    }

    #[must_use]
    pub fn build(self, theme: &ArtisanTheme, selected: bool, content: Element) -> Element {
        let surfaces = Surfaces::of(theme);
        let edge = Edge::of(surfaces.bg, surfaces.ink);
        let highlight = match self.opens() {
            true => vertical(edge.highlight_top, edge.highlight_bottom),
            false => GpuiBackground::Solid(surfaces.bg.to_reference_paint()),
        };
        let padding = self.padding();
        let pad = Row::FormRow.pad();
        let fill = selected.then(|| {
            UiBox::new()
                .style(Hover::Neutral.style(theme))
                .style(
                    Style::new()
                        .absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                        .radius(Length::Px(Row::FormRow.radius_in(CARD))),
                )
                .build()
                .exclude_from_hit_testing()
        });
        let row = UiBox::new()
            .layout("column")
            .style(Style::new().padding_insets(Insets {
                left: pad.x.px(),
                right: pad.x.px(),
                top: pad.y.px(),
                bottom: pad.y.px(),
            }))
            .children(fill)
            .child(content)
            .build();
        let rule = (!self.opens()).then(|| Divider::Rule.build(theme, padding.left - HIGHLIGHT));
        UiBox::new()
            .layout("column")
            .width(Length::Pct(100.0))
            .style(
                Style::new()
                    .corners(self.corners(CARD.radius_px()))
                    .gpui_background(self.rim(&edge))
                    .padding_insets(Insets {
                        left: padding.left,
                        right: padding.right,
                        top: padding.top,
                        bottom: padding.bottom,
                    }),
            )
            .child(self.layer(RIM, highlight))
            .child(self.layer(
                HIGHLIGHT,
                GpuiBackground::Solid(surfaces.bg.to_reference_paint()),
            ))
            .children(rule)
            .child(row)
            .build()
    }
}

fn vertical(top: Oklch, bottom: Oklch) -> GpuiBackground {
    GpuiBackground::LinearGradient {
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
    }
}
