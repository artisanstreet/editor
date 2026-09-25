//! Disclosure flight state and the shared accordion panel builder for
//! [`ConversationSurface`].
//!
//! Extracted verbatim from `conversation_surface.rs` during the phase-3
//! module split; the panel builder was widened to `pub(super)` for
//! sibling-owned render call sites.

use super::*;

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
    /// The clock belongs to the transition, not a temporary element subtree.
    started: Instant,
    /// Keep a closing panel's size independent of nested panels and late rows.
    collapse_height: f32,
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
    /// Each render samples the persisted clock once, so an interrupted flight
    /// starts from the displayed fraction.
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

/// One sample shared by a disclosure's chevron, height and opacity.
pub(super) struct DisclosureFrame {
    panel_state: Entity<DisclosurePanelState>,
    pub(super) fraction: f32,
    flight: Option<DisclosureFlight>,
    content_height: f32,
    open: bool,
}

impl DisclosureFrame {
    pub(super) fn content_mounted(&self) -> bool {
        self.open || self.flight.is_some()
    }
}

/// Sample once before building either the trigger or its panel.
pub(super) fn disclosure_frame(
    selector: &str,
    open: bool,
    motion: MotionPolicy,
    window: &mut Window,
    cx: &mut Context<ConversationSurface>,
) -> DisclosureFrame {
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
            MotionPolicy::Full => Some(DisclosureFlight {
                open,
                from,
                started: Instant::now(),
                collapse_height: content_height,
            }),
            MotionPolicy::Reduced => None,
        };
        panel_state.update(cx, |state, _| {
            state.painted_open = open;
            state.flight = flight;
        });
    }
    if motion == MotionPolicy::Reduced {
        flight = None;
    }
    let mut fraction = if open { 1.0 } else { 0.0 };
    if let Some(armed) = flight {
        let MotionPlan::Animate(animation) = motion.resolve(MotionRecipe::AccordionExpand) else {
            unreachable!("reduced motion disarms the flight");
        };
        let elapsed = armed.started.elapsed();
        if elapsed >= animation.duration() {
            flight = None;
        } else {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "motion curves feed f32 layout values"
            )]
            let eased = animation
                .curve()
                .sample(elapsed.as_secs_f64() / animation.duration().as_secs_f64())
                as f32;
            fraction = armed.from + (armed.target() - armed.from) * eased;
        }
    }
    progress.set(fraction);
    panel_state.update(cx, |state, _| state.flight = flight);
    DisclosureFrame {
        panel_state,
        fraction,
        flight,
        content_height,
        open,
    }
}

/// Attaches the shared accordion panel (height and opacity) to one
/// disclosure root.
///
/// `selector` is the owning element's selector; the panel state, content, and
/// clock identity derive from it, so the session panel and every activity chain
/// use the same transition
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
    reason = "one GPUI builder drives the height/opacity flight through both the controlled and uncontrolled disclosure paths"
)]
pub(super) fn disclosure_flight_panel(
    mut disclosure: Div,
    mut panel: Div,
    content: AnyElement,
    selector: &str,
    frame: DisclosureFrame,
    controlled: bool,
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
        let content_mounted = frame.content_mounted();
        let DisclosureFrame {
            panel_state,
            fraction,
            flight,
            content_height,
            open,
        } = frame;

        // Measure intrinsic content for opening and future transitions. A
        // closing flight uses its captured height: nested tool chains may
        // settle or receive late rows without expanding their closing parent.
        let panel_state_for_prepaint = panel_state.clone();
        panel = panel.on_children_prepainted(move |children_bounds, _, app| {
            if let Some(content_bounds) = children_bounds.first() {
                panel_state_for_prepaint.update(app, |state, _| {
                    state.content_height = f32::from(content_bounds.size.height);
                });
            }
        });

        if content_mounted {
            if let Some(armed) = flight {
                let full_height = if open {
                    content_height
                } else {
                    armed.collapse_height
                };
                // Large tool outputs make a subtree blur costly on Windows.
                // Clipping and opacity retain the accordion reveal without
                // allocating a filtered layer for every frame.
                let inner = inner.opacity(fraction);
                disclosure = disclosure.child(
                    panel
                        .child(inner)
                        .overflow_hidden()
                        .h(px(full_height * fraction)),
                );
                // One persisted clock drives height, opacity and chevron. Rebuilding
                // the subtree cannot restart any part of the transition.
                let surface = cx.entity().downgrade();
                window.on_next_frame(move |_, app| {
                    let _ = surface.update(app, |_, cx| cx.notify());
                });
            } else {
                disclosure = disclosure.child(panel.child(inner));
            }
        } else {
            disclosure = disclosure.child(panel.h(px(0.0)).overflow_hidden());
        }
    } else if frame.open {
        disclosure = disclosure.child(panel.child(inner));
    } else {
        disclosure = disclosure.child(panel.h(px(0.0)).overflow_hidden());
    }

    disclosure.into_any_element()
}
