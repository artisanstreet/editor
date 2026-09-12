//! Disclosure flight state and the shared accordion panel builder for
//! [`ConversationSurface`].
//!
//! Extracted verbatim from `conversation_surface.rs` during the phase-3
//! module split; the panel builder was widened to `pub(super)` for
//! sibling-owned render call sites.

use super::*;

/// The collapsed disclosure panel's inner blur, mirroring `--blur-small`.
const WORK_GROUP_PANEL_BLUR_PX: f32 = 2.0;

/// One work-session disclosure transition: direction plus start fraction.
///
/// The fraction is the expanded share of the panel's measured content height
/// in `0.0..=1.0`. A flight always moves toward [`Self::target`] on the
/// reference 250 ms `cubic-bezier(0.22, 1, 0.36, 1)` clock.
#[derive(Clone, Copy, Debug, PartialEq)]
struct DisclosureFlight {
    /// The open value the flight is moving to.
    open: bool,
    /// The expanded fraction the flight starts from.
    ///
    /// Captured from the last painted frame so an interrupted flight reverses
    /// from the displayed height instead of snapping to an endpoint.
    from: f32,
}

impl DisclosureFlight {
    /// Returns the expanded fraction this flight settles on.
    const fn target(self) -> f32 {
        if self.open { 1.0 } else { 0.0 }
    }
}

/// Window-local disclosure panel state for one work group.
///
/// The reference tweens `grid-template-rows` from zero to the content height;
/// GPUI has no intrinsic track to tween, so the panel's measured content height
/// is the flight target and the panel is clipped to the animated height. A
/// mount rests at its final state exactly like a browser mounting `data-open`
/// without a transition; only an actual open-value change arms a flight, and
/// reduced motion records the change without arming one.
struct DisclosurePanelState {
    /// Intrinsic height of the mounted detail rows in px.
    ///
    /// Updated from prepaint child bounds while content is mounted, so the
    /// last measurement survives a collapsed panel that unmounts its rows.
    content_height: f32,
    /// Open value of the last rendered frame; the flip detector.
    painted_open: bool,
    /// The in-flight transition, if any.
    flight: Option<DisclosureFlight>,
    /// Last painted expanded fraction in `0.0..=1.0`.
    ///
    /// The height animator writes every painted frame, so the next flight can
    /// start from the displayed fraction.
    progress: Rc<Cell<f32>>,
}

impl DisclosurePanelState {
    /// Creates the mount state for one group; no flight is armed at mount.
    fn new(open: bool) -> Self {
        Self {
            content_height: 0.0,
            painted_open: open,
            flight: None,
            progress: Rc::new(Cell::new(if open { 1.0 } else { 0.0 })),
        }
    }
}

/// Builds one disclosure flight clock: 250 ms
/// `cubic-bezier(0.22, 1, 0.36, 1)`, sampled by [`MotionCurve::SmoothOut`].
///
/// The curve is sampled inside the animator rather than installed as GPUI
/// easing, mirroring the other local clocks in this surface. The panel and its
/// inner share the clock shape, so height, opacity, and blur all ride the
/// reference's single 250 ms ease.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the disclosure curve samples in f64 and feeds the f32 animation clock; the narrowing is the intended precision"
)]
fn disclosure_flight_animation(plan: MotionPlan) -> Animation {
    let MotionPlan::Animate(animation) = plan else {
        unreachable!("a disclosure flight only runs under full motion");
    };
    let curve = animation.curve();
    Animation::new(animation.duration())
        .with_easing(move |progress: f32| curve.sample(f64::from(progress)) as f32)
}

/// Attaches the shared accordion panel (height, opacity, blur) to one
/// disclosure root.
///
/// `selector` is the owning element's selector; the panel state, content, and
/// animator identities derive from it exactly as the work-session disclosure
/// always has, so the session panel and every activity chain ride one clock
/// implementation. `controlled` follows the existing work-group policy:
/// uncontrolled roots paint their content statically, while controlled roots
/// keep content mounted only while it is visible or a collapse is still
/// measuring. The caller owns the root and panel debug selectors and the
/// trigger child; this helper owns the content selector and flight.
#[expect(
    clippy::too_many_arguments,
    reason = "the panel assembles the disclosure root, content, identity, and motion policy in one place; bundling would obscure the relation between them"
)]
#[expect(
    clippy::too_many_lines,
    reason = "one GPUI builder drives the height/opacity/blur flight through both the controlled and uncontrolled disclosure paths"
)]
pub(super) fn disclosure_flight_panel(
    mut disclosure: Div,
    mut panel: Div,
    content: AnyElement,
    selector: &str,
    open: bool,
    controlled: bool,
    motion: MotionPolicy,
    window: &mut Window,
    cx: &mut Context<ConversationSurface>,
) -> AnyElement {
    let inner = div()
        .w_full()
        .min_w_0()
        .flex_shrink_0()
        .debug_selector({
            let content_selector = format!("{selector}-disclosure-content");
            move || content_selector.clone()
        })
        .child(content);

    if controlled {
        let panel_state = window.use_keyed_state(
            ElementId::Name(SharedString::from(format!(
                "{selector}-disclosure-panel-state"
            ))),
            cx,
            |_, _| DisclosurePanelState::new(open),
        );
        let (painted_open, mut flight, progress, content_height) = {
            let state = panel_state.read(cx);
            (
                state.painted_open,
                state.flight,
                state.progress.clone(),
                state.content_height,
            )
        };
        if painted_open != open {
            let from = progress.get().clamp(0.0, 1.0);
            flight = match motion {
                MotionPolicy::Full => Some(DisclosureFlight { open, from }),
                MotionPolicy::Reduced => None,
            };
            panel_state.update(cx, |state, _| {
                state.painted_open = open;
                state.flight = flight;
            });
        }
        let animated_flight =
            flight.filter(|armed| armed.open == open && motion == MotionPolicy::Full);
        if animated_flight.is_none() && flight.is_some() {
            // A policy switch midway through a flight (or a reduced-motion
            // render) settles at the final state now; the clock is dropped
            // so it can never replay when full motion returns.
            flight = None;
            panel_state.update(cx, |state, _| state.flight = None);
        }
        let content_mounted = open || matches!(flight, Some(armed) if !armed.open);

        // Height measurement rides the panel's prepaint boundary, after
        // this frame's animator has written its fraction: a completed
        // flight disarms here, which unmounts a settled collapse's rows on
        // the next frame without ever dropping the stored target height.
        let panel_state_for_prepaint = panel_state.clone();
        panel = panel.on_children_prepainted(move |children_bounds, _, app| {
            let Some(content_bounds) = children_bounds.first() else {
                return;
            };
            panel_state_for_prepaint.update(app, |state, state_cx| {
                state.content_height = f32::from(content_bounds.size.height);
                if state
                    .flight
                    .is_some_and(|flight| (state.progress.get() - flight.target()).abs() <= 1e-4)
                {
                    state.flight = None;
                    state_cx.notify();
                }
            });
        });

        if content_mounted {
            if let Some(armed) = animated_flight {
                let (from, target) = (armed.from, armed.target());
                let full_height = content_height;
                let progress_for_height = progress.clone();
                let inner_animation =
                    disclosure_flight_animation(motion.resolve(MotionRecipe::AccordionExpand));
                let inner = inner
                    .with_animations(
                        ElementId::Name(SharedString::from(format!(
                            "{selector}-disclosure-content-{}",
                            if open { "open" } else { "closed" }
                        ))),
                        vec![inner_animation],
                        move |inner, _index, value| {
                            let fraction = from + (target - from) * value;
                            if fraction >= 1.0 {
                                inner.opacity(1.0)
                            } else {
                                inner.opacity(fraction).filter(vec![Filter::Blur(px(
                                    WORK_GROUP_PANEL_BLUR_PX * (1.0 - fraction),
                                ))])
                            }
                        },
                    )
                    .into_any_element();
                let panel_animation =
                    disclosure_flight_animation(motion.resolve(MotionRecipe::AccordionExpand));
                disclosure = disclosure.child(
                    panel
                        .child(inner)
                        .overflow_hidden()
                        .with_animations(
                            ElementId::Name(SharedString::from(format!(
                                "{selector}-disclosure-panel-{}",
                                if open { "open" } else { "closed" }
                            ))),
                            vec![panel_animation],
                            move |panel, _index, value| {
                                let fraction = from + (target - from) * value;
                                progress_for_height.set(fraction);
                                panel.h(px(full_height * fraction))
                            },
                        )
                        .into_any_element(),
                );
            } else {
                // A settled flight disarms above, so mounted content
                // outside a flight is always the open side.
                debug_assert!(open, "mounted content outside a flight is open");
                progress.set(1.0);
                disclosure = disclosure.child(panel.child(inner));
            }
        } else {
            progress.set(if open { 1.0 } else { 0.0 });
            disclosure = disclosure.child(panel.h(px(0.0)).overflow_hidden());
        }
    } else if open {
        disclosure = disclosure.child(panel.child(inner));
    } else {
        disclosure = disclosure.child(panel.h(px(0.0)).overflow_hidden());
    }

    disclosure.into_any_element()
}
