//! Scroll edges.
//!
//! The surface's colour fades over a scrolled list's top or bottom, so text
//! dissolves instead of being cut off by the edge. An edge
//! shows only while content is hidden past it, and fades in and out.

use std::{cell::Cell, rc::Rc, time::Instant};

use gpui::{Canvas, ListState, Styled as _, canvas, px};

use crate::{
    gradient::vertical_gradient,
    motion::{MotionDuration, MotionPolicy},
    theme::Oklch,
};

/// The band's height.
pub const SCROLL_EDGE_HEIGHT: f32 = 24.0;
/// How much content must hide past the edge before the band is fully
/// shown: a list resting at its edge has nothing to hide, so no band.
const SCROLL_EDGE_RAMP: f32 = 16.0;

/// Which edge of the list the band sits on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Edge {
    Top,
    Bottom,
}

/// The band's shown amount, kept by its owner across renders so it can
/// fade toward where the scroll position puts it.
#[derive(Clone, Default)]
pub struct EdgeFade(Rc<Cell<FadeState>>);

#[derive(Clone, Copy, Default)]
struct FadeState {
    shown: f32,
    /// The last animated frame; `None` while settled.
    at: Option<Instant>,
}

impl EdgeFade {
    /// Moves toward `target` by the time since the last frame; whether it
    /// is still moving.
    fn step(&self, target: f32, reduced: bool) -> (f32, bool) {
        let state = self.0.get();
        let now = Instant::now();
        let shown = match state.at {
            _ if reduced => target,
            // A fade starts from where it settled, not from the time since.
            None => state.shown,
            Some(at) => {
                let tau = MotionDuration::Fast.as_duration().as_secs_f32() / 4.0;
                let t = 1.0 - (-now.duration_since(at).as_secs_f32() / tau).exp();
                (target - state.shown).mul_add(t, state.shown)
            }
        };
        let settled = (target - shown).abs() < 0.01;
        let shown = if settled { target } else { shown };
        self.0.set(FadeState {
            shown,
            at: (!settled).then_some(now),
        });
        (shown, !settled)
    }
}

/// How far `list`'s content runs past its top edge; infinite past its
/// first row.
#[must_use]
pub fn hidden_above(list: &ListState) -> f32 {
    let top = list.logical_scroll_top();
    if top.item_ix > 0 {
        f32::INFINITY
    } else {
        f32::from(top.offset_in_item)
    }
}

/// How far `list`'s content runs past its bottom edge. An unmeasured last
/// row is infinitely far past it.
#[must_use]
pub fn hidden_below(list: &ListState) -> f32 {
    let Some(final_row) = list.item_count().checked_sub(1) else {
        return 0.0;
    };
    list.bounds_for_item(final_row)
        .map_or(f32::INFINITY, |row| {
            f32::from(row.bottom() - list.viewport_bounds().bottom())
        })
}

/// The band on `edge`, in `color`.
///
/// It shows by how much content `hidden` reports past it. Painted from the scroll position on each frame, and
/// faded paint-only, so neither scrolling nor the fade re-renders the rows.
/// The caller places it: absolute, over the list, on its edge.
pub fn scroll_edge(
    edge: Edge,
    color: Oklch,
    fade: &EdgeFade,
    hidden: impl Fn() -> f32 + 'static,
) -> Canvas<()> {
    let fade = fade.clone();
    canvas(
        |_, _, _| {},
        move |bounds, (), window, cx| {
            window.paint_animated(
                move |window, cx| {
                    let target = (hidden() / SCROLL_EDGE_RAMP).clamp(0.0, 1.0);
                    let reduced = MotionPolicy::current(cx) == MotionPolicy::Reduced;
                    let (shown, moving) = fade.step(target, reduced);
                    if shown > 0.0 {
                        let (solid, clear) = (color.fade(shown), color.fade(0.0));
                        let fill = match edge {
                            Edge::Top => vertical_gradient(solid, clear),
                            Edge::Bottom => vertical_gradient(clear, solid),
                        };
                        window.paint_quad(gpui::fill(bounds, fill));
                    }
                    moving
                },
                cx,
            );
        },
    )
    .absolute()
    .left_0()
    .w_full()
    .h(px(SCROLL_EDGE_HEIGHT))
}
