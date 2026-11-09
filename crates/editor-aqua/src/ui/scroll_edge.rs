use super::motion::MotionDuration;
use super::theme::Oklch;
use aqua_core::NodeId;
use aqua_ui::element::{Box as UiBox, Element, Key};
use aqua_ui::style::{LayoutStyle, Length, Style};
use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

pub const SCROLL_EDGE_HEIGHT: f32 = 24.0;
const SCROLL_EDGE_RAMP: f32 = 16.0;
const SETTLED: f32 = 0.01;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Edge {
    Top,
    Bottom,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EdgePaint {
    Gradient,
    InsetShadow,
}

#[must_use]
pub fn target(hidden: f32) -> f32 {
    (hidden / SCROLL_EDGE_RAMP).clamp(0.0, 1.0)
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EdgeFade {
    shown: f32,
    at: Option<Instant>,
}

impl EdgeFade {
    #[must_use]
    pub const fn shown(self) -> f32 {
        self.shown
    }

    #[must_use]
    pub fn step(self, target: f32, reduced: bool, now: Instant) -> (Self, bool) {
        let shown = match self.at {
            _ if reduced => target,
            None => self.shown,
            Some(at) => {
                let tau = MotionDuration::Fast.as_duration().as_secs_f32() / 4.0;
                let t = 1.0 - (-now.saturating_duration_since(at).as_secs_f32() / tau).exp();
                (target - self.shown).mul_add(t, self.shown)
            }
        };
        let settled = (target - shown).abs() < SETTLED;
        let shown = if settled { target } else { shown };
        (
            Self {
                shown,
                at: (!settled).then_some(now),
            },
            !settled,
        )
    }
}

#[derive(Clone, Default)]
pub struct ScrollEdges {
    fades: Rc<[Cell<EdgeFade>; 2]>,
}

impl PartialEq for ScrollEdges {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for ScrollEdges {}

fn slot(edge: Edge) -> usize {
    match edge {
        Edge::Top => 0,
        Edge::Bottom => 1,
    }
}

fn gradient(edge: Edge, color: Oklch, shown: f32) -> Style {
    let (solid, clear) = (color.fade(shown), color.fade(0.0));
    let (top, bottom) = match edge {
        Edge::Top => (solid, clear),
        Edge::Bottom => (clear, solid),
    };
    Style::new().gpui_background(aqua_core::GpuiBackground::LinearGradient {
        angle: 180.0,
        color_space: aqua_core::GpuiGradientColorSpace::Oklab,
        stops: [
            aqua_core::GpuiGradientStop {
                percentage: 0.0,
                color: top.to_reference_paint(),
            },
            aqua_core::GpuiGradientStop {
                percentage: 1.0,
                color: bottom.to_reference_paint(),
            },
        ],
    })
}

fn inset_shadow(edge: Edge, color: Oklch, shown: f32) -> Style {
    let reach = SCROLL_EDGE_HEIGHT * 1.5;
    let dy = match edge {
        Edge::Top => reach,
        Edge::Bottom => -reach,
    };
    Style::new().gpui_shadows(vec![aqua_ui::style::GpuiShadowLayer {
        dx: 0.0,
        dy,
        blur: SCROLL_EDGE_HEIGHT,
        spread: -SCROLL_EDGE_HEIGHT,
        inset: true,
        color: color.fade(shown).to_reference_paint(),
    }])
}

fn edge_style(paint: EdgePaint, edge: Edge, color: Oklch, shown: f32) -> Style {
    match paint {
        EdgePaint::Gradient => gradient(edge, color, shown),
        EdgePaint::InsetShadow => inset_shadow(edge, color, shown),
    }
}

impl ScrollEdges {
    fn fade(&self, edge: Edge) -> EdgeFade {
        self.fades[slot(edge)].get()
    }

    pub fn reset(&self) {
        self.fades
            .iter()
            .for_each(|fade| fade.set(EdgeFade::default()));
    }

    pub fn band(&self, key: impl Into<Key>, edge: Edge, color: Oklch) -> Element {
        let placement = match edge {
            Edge::Top => Some(0.0),
            Edge::Bottom => None,
        };
        let bottom = match edge {
            Edge::Top => None,
            Edge::Bottom => Some(0.0),
        };
        UiBox::new()
            .width(Length::Pct(100.0))
            .height(SCROLL_EDGE_HEIGHT)
            .style(
                gradient(edge, color, self.fade(edge).shown())
                    .absolute_at(Some(0.0), placement, Some(0.0), bottom)
                    .flex_shrink(0.0),
            )
            .build()
            .with_key(key)
            .exclude_from_hit_testing()
    }

    pub fn inset_band(
        &self,
        key: impl Into<Key>,
        edge: Edge,
        color: Oklch,
        radius: f32,
    ) -> Element {
        UiBox::new()
            .radius(radius)
            .style(
                inset_shadow(edge, color, self.fade(edge).shown())
                    .absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0)),
            )
            .build()
            .with_key(key)
            .exclude_from_hit_testing()
    }

    pub fn advance(
        &self,
        ui: &mut aqua::UiApp,
        edge: Edge,
        paint_kind: EdgePaint,
        band: NodeId,
        color: Oklch,
        hidden: f32,
        reduced: bool,
        now: Instant,
    ) -> bool {
        let (fade, moving) = self.fade(edge).step(target(hidden), reduced, now);
        let previous = self.fade(edge);
        self.fades[slot(edge)].set(fade);
        if previous.shown() != fade.shown() {
            if let Some(mut paint) = ui.tree().paint(band) {
                edge_style(paint_kind, edge, color, fade.shown())
                    .apply_to(&mut LayoutStyle::default(), &mut paint);
                ui.tree_mut().set_paint(band, paint);
            }
        }
        moving
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hidden {
    pub above: f32,
    pub below: f32,
}

#[must_use]
pub fn hidden_in(ui: &aqua::UiApp, scroller: NodeId) -> Option<Hidden> {
    let offset = ui.tree().scroll_offset(scroller)?;
    let viewport = ui.tree().rect(scroller)?;
    let content = ui.tree().content_size(scroller)?;
    let max = (content.h - viewport.h).max(0.0);
    Some(Hidden {
        above: offset.y.max(0.0),
        below: (max - offset.y).max(0.0),
    })
}

#[must_use]
pub fn keyed(ui: &aqua::UiApp, key: &str) -> Option<NodeId> {
    let key = aqua_ui::tree::hash_key(Key::from(key));
    ui.tree().sole_node_with_key(key)
}

pub struct EdgeSurface<'a> {
    pub edges: &'a ScrollEdges,
    pub scroller: &'a str,
    pub top: &'a str,
    pub bottom: &'a str,
    pub top_color: Oklch,
    pub bottom_color: Oklch,
    pub paint: EdgePaint,
}

impl EdgeSurface<'_> {
    pub fn advance(&self, ui: &mut aqua::UiApp, reduced: bool, now: Instant) -> bool {
        let scroller = keyed(ui, self.scroller).or_else(|| ui.find_debug_selector(self.scroller));
        let Some(hidden) = scroller.and_then(|node| hidden_in(ui, node)) else {
            return false;
        };
        let top = keyed(ui, self.top);
        let bottom = keyed(ui, self.bottom);
        let upper = top.is_some_and(|band| {
            self.edges.advance(
                ui,
                Edge::Top,
                self.paint,
                band,
                self.top_color,
                hidden.above,
                reduced,
                now,
            )
        });
        let lower = bottom.is_some_and(|band| {
            self.edges.advance(
                ui,
                Edge::Bottom,
                self.paint,
                band,
                self.bottom_color,
                hidden.below,
                reduced,
                now,
            )
        });
        upper || lower
    }
}
