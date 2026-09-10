//! Rust-native GPUI host for one controller-owned conversation scene.
//!
//! [`ConversationStateController`] remains the only conversation policy
//! owner. This entity owns the GPUI child and the boundaries around it: typed
//! surface-action routing, the last accepted replacement scene, and a bounded
//! FIFO of effects for an outer application/window adapter. It does not
//! perform transport I/O or acknowledge a scroll synchronously.
//!
//! Controller effects are moved only as a complete prefix when the host
//! outbox has room. If the host outbox is full, the controller keeps its
//! bounded pending effects, so an accepted controller event is never reported
//! as failed merely because an adapter is applying backpressure.

#![forbid(unsafe_code)]

use artisan_domain::{IdentifierError, ThreadId, TurnId, account_usage::iso_millis};
use artisan_ui::theme::ThemeMode;
use gpui::{
    App, AppContext as _, ClipboardItem, Context, Entity, IntoElement, Render, Subscription,
    Window,
};
use thiserror::Error;
use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::conversation_delivery_machine::ConversationDeliveryEffect;
use crate::conversation_relative_age::format_relative_age;
use crate::conversation_scene::{ConversationScene, TurnBlock};
use crate::conversation_state_machine::{
    ConversationStateController, ConversationStateEffect, ConversationStateError,
    ConversationStateEvent, ConversationStateView, MAX_PENDING_EFFECTS,
};
use crate::conversation_steering_machine::SteeringEffect;
use crate::conversation_surface::{
    ConversationSurface, ConversationSurfaceAction, ConversationSurfaceTarget,
};
use crate::conversation_turn_footer_policy::{
    ConversationTurnFooterPolicy, CopyOutcome, TurnFooterAction, TurnFooterInput,
};
use crate::conversation_view_machine::{ViewportEffect, ViewportEvent};

/// Maximum number of effects exported by one host before adapter backpressure
/// stops further direct export.
///
/// This matches the controller's own maximum pending-effect count. Keeping
/// the host bound at least as large as the controller bound lets the host
/// transfer any complete controller outbox without draining and losing a
/// tail. The application boundary may apply a second bounded queue.
pub const CONVERSATION_HOST_MAX_EFFECTS: usize = MAX_PENDING_EFFECTS;

/// Typed, redacted refusal retained in the host effect outbox.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConversationHostRefusal {
    /// A controller event was refused without changing accepted controller
    /// state. The aggregate error contains only typed bounded identities and
    /// diagnosis, never message bodies or provider payloads.
    Controller(ConversationStateError),
    /// The host could not retain a refusal because an older effect prefix is
    /// still waiting for the outer adapter.
    EffectOutboxFull { count: usize, maximum: usize },
    /// Host construction could not derive its initial authoritative scene.
    Initialization(ConversationHostError),
}

/// Typed effects crossing the host/application boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConversationHostEffect {
    /// An effect emitted by the aggregate controller, retained in aggregate
    /// FIFO order.
    Controller(ConversationStateEffect),
    /// A surface request that belongs to the outer viewport/window adapter.
    /// No GPUI scroll completion is claimed here.
    ScrollIntent {
        /// Stable scene or item target.
        target: ConversationSurfaceTarget,
    },
    /// A typed refusal that could be observed by the outer adapter.
    Refused {
        /// Redacted refusal diagnosis.
        refusal: ConversationHostRefusal,
    },
}

/// Typed host construction, dispatch, and backpressure error.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ConversationHostError {
    /// The aggregate refused an event.
    #[error("conversation host controller refused an event: {0}")]
    Controller(#[source] ConversationStateError),
    /// A refusal could not bypass older effects waiting in a bounded queue.
    #[error(
        "conversation host could not retain a refusal with {count} pending effects; maximum is {maximum}"
    )]
    EffectOutboxFull { count: usize, maximum: usize },
    /// The accepted controller state could not be projected into a scene.
    #[error("conversation host scene projection failed: {0}")]
    SceneProjection(#[source] ConversationStateError),
    /// A host integration supplied an invalid fixed thread identity.
    #[error("conversation host thread identity was invalid: {0}")]
    InvalidThreadId(#[source] IdentifierError),
}

enum SurfaceRouteDecision {
    Accepted,
    Backpressured,
}

/// Cadence of the host clock mirror while an authoritative turn is active.
///
/// One frame-time sample per second matches the reference work-session tick;
/// the task runs only while the accepted scene carries active work and stops
/// on settlement or host drop.
const CLOCK_TICK_INTERVAL: Duration = Duration::from_secs(1);

/// Samples the host clock as Unix millis for footer ages and frame time.
///
/// A clock failure floors to zero rather than failing the caller; both
/// consumers clamp forward.
#[must_use]
pub fn host_now_millis() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis())
            .unwrap_or(0),
    )
    .unwrap_or(i64::MAX)
}

/// Returns whether the accepted scene carries authoritative active work.
///
/// The scene is the authority: a ticking clock exists only while a status
/// block narrates active work, and stops the moment the scene settles.
#[must_use]
pub fn scene_has_active_work(scene: &ConversationScene) -> bool {
    scene.turn_scenes().iter().any(|turn| {
        turn.blocks().iter().any(|block| {
            matches!(block, TurnBlock::TurnStatus(status) if status.narration.is_active_work())
        })
    })
}

/// Returns the settled footer's machine-readable timestamp and exact response
/// bytes for one turn, if that turn paints a footer.
///
/// The timestamp uses the shared [`iso_millis`] formatter so the machine
/// fact and the relative-age adapter input are one value. The scene
/// settlement is authoritative; the action echo is never used.
#[must_use]
pub fn settled_footer_for(scene: &ConversationScene, turn_id: &TurnId) -> Option<(String, String)> {
    let turn = scene.turn_scene(turn_id)?;
    turn.blocks().iter().find_map(|block| match block {
        TurnBlock::TurnFooter(footer) => footer.settlement.as_ref().map(|settlement| {
            (
                iso_millis(settlement.settled_at_ms()),
                settlement.response_text().to_owned(),
            )
        }),
        _ => None,
    })
}

/// Returns whether a cached footer policy still matches the canonical
/// settlement facts.
///
/// A revised settlement (new response bytes or timestamp after an accepted
/// revision) retires the cached policy so a later reveal or copy can never
/// serve stale bytes.
#[must_use]
pub fn footer_policy_is_stale(
    policy: &ConversationTurnFooterPolicy,
    settled_at: &str,
    response_text: &str,
) -> bool {
    policy.settled_at() != settled_at || policy.response_text() != response_text
}

/// The native host for one fixed conversation thread.
pub struct ConversationHost {
    controller: ConversationStateController,
    surface: Entity<ConversationSurface>,
    /// Kept for the complete host lifetime so surface notifications continue
    /// to route through the controller boundary.
    _surface_subscription: Subscription,
    effects: Vec<ConversationHostEffect>,
    pending_extent_changes: usize,
    /// Per-turn footer policies keyed by owning turn.
    ///
    /// Each policy owns one settled footer's clock-sample and clipboard
    /// flow through the existing [`ConversationTurnFooterPolicy`]; entries
    /// are pruned to scene turns on every accepted scene replacement.
    footer_policies: HashMap<TurnId, ConversationTurnFooterPolicy>,
    /// Whether the bounded clock task is currently running.
    clock_running: bool,
    /// Retained clock task; dropping the host drops the task with it.
    clock_task: Option<gpui::Task<()>>,
}

impl ConversationHost {
    /// Creates a host value inside an already-created GPUI entity.
    ///
    /// The initial scene is projected before the child surface is constructed;
    /// callers can therefore handle the only fallible construction step
    /// without a production panic. [`Self::mount`] is the convenient two-stage
    /// entity factory used by application hosts.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationHostError::SceneProjection`] if the fresh
    /// controller cannot produce its empty initial scene.
    pub fn new(
        thread_id: ThreadId,
        theme_mode: ThemeMode,
        cx: &mut Context<Self>,
    ) -> Result<Self, ConversationHostError> {
        let controller = ConversationStateController::new(thread_id);
        let scene = controller
            .scene()
            .map_err(ConversationHostError::SceneProjection)?;
        let surface = cx.new(|surface_cx| ConversationSurface::new(scene, theme_mode, surface_cx));
        Ok(Self::from_parts(controller, surface, cx))
    }

    /// Creates and registers a genuine [`ConversationHost`] entity in the
    /// application context.
    ///
    /// This two-stage factory keeps construction fallible without asking an
    /// infallible GPUI entity initializer to panic on scene projection.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationHostError::SceneProjection`] if the fresh
    /// controller cannot produce its empty initial scene.
    pub fn mount(
        thread_id: ThreadId,
        theme_mode: ThemeMode,
        cx: &mut App,
    ) -> Result<Entity<Self>, ConversationHostError> {
        let controller = ConversationStateController::new(thread_id);
        let scene = controller
            .scene()
            .map_err(ConversationHostError::SceneProjection)?;
        let surface = cx.new(|surface_cx| ConversationSurface::new(scene, theme_mode, surface_cx));
        Ok(cx.new(|host_cx| Self::from_parts(controller, surface, host_cx)))
    }

    fn from_parts(
        controller: ConversationStateController,
        surface: Entity<ConversationSurface>,
        cx: &mut Context<Self>,
    ) -> Self {
        let surface_subscription = cx.observe(&surface, |host, surface, cx| {
            host.route_surface_actions(&surface, cx);
        });
        let mut host = Self {
            controller,
            surface,
            _surface_subscription: surface_subscription,
            effects: Vec::with_capacity(CONVERSATION_HOST_MAX_EFFECTS),
            pending_extent_changes: 0,
            footer_policies: HashMap::new(),
            clock_running: false,
            clock_task: None,
        };
        host.flush_controller_effects();
        host
    }

    /// Returns the immutable controller view, including controller-side
    /// pending-effect count when the host outbox is applying backpressure.
    #[must_use]
    pub fn controller_view(&self) -> ConversationStateView {
        self.controller.view()
    }

    /// Re-projects the controller's authoritative scene for diagnostics and
    /// black-box verification. The rendered child is replaced only by the
    /// same projection during accepted invalidating dispatches.
    ///
    /// # Errors
    ///
    /// Returns the controller's typed scene-projection refusal.
    pub fn controller_scene(&self) -> Result<ConversationScene, ConversationHostError> {
        self.controller
            .scene()
            .map_err(ConversationHostError::SceneProjection)
    }

    /// Returns the one child surface entity.
    #[must_use]
    pub fn surface(&self) -> &Entity<ConversationSurface> {
        &self.surface
    }

    /// Returns effects already exported to the outer adapter in FIFO order.
    #[must_use]
    pub fn pending_effects(&self) -> &[ConversationHostEffect] {
        &self.effects
    }

    /// Returns the number of effects already exported to the outer adapter.
    #[must_use]
    pub fn pending_effect_count(&self) -> usize {
        self.effects.len()
    }

    /// Returns the number of accepted controller effects still held by the
    /// controller because the host outbox could not accept their complete
    /// prefix.
    #[must_use]
    pub fn pending_controller_effect_count(&self) -> usize {
        self.controller.pending_effect_count()
    }

    /// Returns the total number of effects across the host and controller
    /// FIFO segments.
    #[must_use]
    pub fn total_pending_effect_count(&self) -> usize {
        self.effects
            .len()
            .saturating_add(self.controller.pending_effect_count())
            .saturating_add(self.pending_extent_changes)
    }

    /// Drains the currently exported host-effect prefix.
    ///
    /// Before returning, the next complete controller-effect prefix is moved
    /// into the now-empty host outbox. A caller applying a second bounded
    /// queue can therefore drain repeatedly; any controller tail remains
    /// observable through [`Self::pending_controller_effect_count`] and is
    /// never silently discarded.
    #[must_use]
    pub fn drain_effects(&mut self) -> Vec<ConversationHostEffect> {
        let effects = std::mem::replace(
            &mut self.effects,
            Vec::with_capacity(CONVERSATION_HOST_MAX_EFFECTS),
        );
        self.flush_controller_effects();
        let _ = self.dispatch_pending_extent_changes();
        self.flush_controller_effects();
        effects
    }

    /// Retries the oldest surface action after an outer adapter has drained
    /// host effects.
    ///
    /// The stored surface subscription handles ordinary notifications. This
    /// explicit retry is the bounded backpressure seam for an adapter that
    /// drained effects without causing a new surface notification.
    pub fn process_pending_actions(&mut self, cx: &mut Context<Self>) {
        let _ = self.dispatch_pending_extent_changes();
        self.flush_controller_effects();
        let surface = self.surface.clone();
        surface.update(cx, |surface, surface_cx| {
            surface.retry_pending_viewport_observation(surface_cx);
        });
        self.route_surface_actions(&surface, cx);
    }

    /// Dispatches one typed aggregate event.
    ///
    /// Every accepted event is acknowledged as accepted even when its
    /// controller effects remain in the controller's bounded outbox. A
    /// controller refusal is retained as a typed host effect when the older
    /// FIFO prefix permits it; otherwise a typed host error applies
    /// backpressure and no surface action is acknowledged.
    ///
    /// # Errors
    ///
    /// Returns a typed controller refusal, a typed refusal-backpressure error,
    /// or a typed scene-projection error. No error path panics.
    pub fn dispatch(
        &mut self,
        event: ConversationStateEvent,
        cx: &mut Context<Self>,
    ) -> Result<(), ConversationHostError> {
        self.flush_controller_effects();
        if let Err(error) = self.dispatch_pending_extent_changes() {
            self.flush_controller_effects();
            return Err(
                if matches!(&error, ConversationStateError::CapacityExhausted { .. }) {
                    ConversationHostError::EffectOutboxFull {
                        count: self.total_pending_effect_count(),
                        maximum: CONVERSATION_HOST_MAX_EFFECTS,
                    }
                } else {
                    ConversationHostError::Controller(error)
                },
            );
        }
        self.flush_controller_effects();
        let before = self.controller.pending_effect_count();
        match self.controller.dispatch(event) {
            Ok(()) => {
                let requires_extent_change = self.controller_effects_require_extent_change(before);
                if self.controller_effects_invalidate_render(before) {
                    let scene = match self.controller.scene() {
                        Ok(scene) => scene,
                        Err(error) => {
                            self.flush_controller_effects();
                            cx.notify();
                            return Err(ConversationHostError::SceneProjection(error));
                        }
                    };
                    let live_turns: Vec<TurnId> = scene
                        .turn_scenes()
                        .iter()
                        .map(|turn| turn.turn_id.clone())
                        .collect();
                    let clock_wanted = scene_has_active_work(&scene);
                    self.surface.update(cx, |surface, surface_cx| {
                        surface.replace_scene(scene, surface_cx);
                    });
                    self.footer_policies
                        .retain(|turn_id, _| live_turns.contains(turn_id));
                    self.reconcile_clock(clock_wanted, cx);
                }
                if requires_extent_change {
                    self.pending_extent_changes = self.pending_extent_changes.saturating_add(1);
                    self.flush_controller_effects();
                    let _ = self.dispatch_pending_extent_changes();
                }
                self.flush_controller_effects();
                cx.notify();
                Ok(())
            }
            Err(error) => {
                self.flush_controller_effects();
                let retained = self.retain_refusal(error.clone());
                cx.notify();
                if retained {
                    Err(ConversationHostError::Controller(error))
                } else {
                    Err(ConversationHostError::EffectOutboxFull {
                        count: self.total_pending_effect_count(),
                        maximum: CONVERSATION_HOST_MAX_EFFECTS,
                    })
                }
            }
        }
    }

    fn route_surface_actions(
        &mut self,
        surface: &Entity<ConversationSurface>,
        cx: &mut Context<Self>,
    ) {
        self.flush_controller_effects();
        let _ = self.dispatch_pending_extent_changes();
        self.flush_controller_effects();
        surface.update(cx, |surface, surface_cx| {
            surface.retry_pending_viewport_observation(surface_cx);
        });
        while let Some(action) = surface.read(cx).next_action().cloned() {
            let decision = match action {
                ConversationSurfaceAction::DisclosureToggleRequested { id, requested_open } => self
                    .route_controller_event(
                        ConversationStateEvent::Disclosure {
                            scene_id: id,
                            event: if requested_open {
                                crate::conversation_view_machine::DisclosureEvent::UserOpen
                            } else {
                                crate::conversation_view_machine::DisclosureEvent::UserClose
                            },
                        },
                        cx,
                    ),
                ConversationSurfaceAction::ViewportObserved(observation) => self
                    .route_controller_event(
                        ConversationStateEvent::Viewport(ViewportEvent::UserScrolled {
                            at_bottom: observation.at_bottom,
                        }),
                        cx,
                    ),
                ConversationSurfaceAction::JumpToLatestRequested => self.route_controller_event(
                    ConversationStateEvent::Viewport(ViewportEvent::JumpToBottomRequested),
                    cx,
                ),
                ConversationSurfaceAction::ScrollIntent { target } => {
                    self.route_scroll_intent(target, cx)
                }
                ConversationSurfaceAction::TurnFooterRevealed { turn } => {
                    self.route_footer_revealed(turn, surface, cx)
                }
                ConversationSurfaceAction::TurnFooterCopyRequested { turn, text: _ } => {
                    self.route_footer_copy(turn, surface, cx)
                }
            };

            match decision {
                SurfaceRouteDecision::Accepted => {
                    surface.update(cx, |surface, _| {
                        let _ = surface.take_next_action();
                    });
                }
                SurfaceRouteDecision::Backpressured => break,
            }
        }
        surface.update(cx, |surface, surface_cx| {
            surface.retry_pending_viewport_observation(surface_cx);
        });
        let _ = self.dispatch_pending_extent_changes();
        self.flush_controller_effects();
    }

    fn route_controller_event(
        &mut self,
        event: ConversationStateEvent,
        cx: &mut Context<Self>,
    ) -> SurfaceRouteDecision {
        match self.dispatch(event, cx) {
            Ok(())
            | Err(
                ConversationHostError::Controller(_)
                | ConversationHostError::SceneProjection(_)
                | ConversationHostError::InvalidThreadId(_),
            ) => SurfaceRouteDecision::Accepted,
            Err(ConversationHostError::EffectOutboxFull { .. }) => {
                SurfaceRouteDecision::Backpressured
            }
        }
    }

    fn route_scroll_intent(
        &mut self,
        target: ConversationSurfaceTarget,
        cx: &mut Context<Self>,
    ) -> SurfaceRouteDecision {
        self.flush_controller_effects();
        if self.controller.pending_effect_count() > 0
            || self.effects.len() >= CONVERSATION_HOST_MAX_EFFECTS
        {
            return SurfaceRouteDecision::Backpressured;
        }
        self.effects
            .push(ConversationHostEffect::ScrollIntent { target });
        cx.notify();
        SurfaceRouteDecision::Accepted
    }

    /// Starts or stops the bounded clock mirror from the scene's authority.
    ///
    /// A running task is never restarted while work stays active, so the
    /// one-second cadence holds without a busy loop. Stopping clears the
    /// mirrored frame time immediately; the retained task exits on its next
    /// tick, and host drop ends it through the failed entity update.
    fn reconcile_clock(&mut self, active: bool, cx: &mut Context<Self>) {
        if active == self.clock_running {
            return;
        }
        self.clock_running = active;
        if active {
            // Push the current frame time at once so capture and first paint
            // already say `Thinking/Working for X` instead of the bare verb.
            let now_ms = host_now_millis();
            self.surface.update(cx, |surface, surface_cx| {
                surface.set_active_now_ms(Some(now_ms), surface_cx);
            });
            let task = cx.spawn(async move |host, cx| {
                loop {
                    cx.background_executor().timer(CLOCK_TICK_INTERVAL).await;
                    let tick = host.update(cx, |host, cx| host.tick_clock(cx)).ok();
                    if tick != Some(true) {
                        break;
                    }
                }
            });
            self.clock_task = Some(task);
        } else {
            self.clock_task = None;
            self.surface.update(cx, |surface, surface_cx| {
                surface.set_active_now_ms(None, surface_cx);
            });
        }
    }

    /// Mirrors one frame-time sample while the clock is wanted.
    ///
    /// Returns whether the task should continue: false stops it at this tick.
    fn tick_clock(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.clock_running {
            return false;
        }
        let now_ms = host_now_millis();
        self.surface.update(cx, |surface, surface_cx| {
            surface.set_active_now_ms(Some(now_ms), surface_cx);
        });
        true
    }

    /// Returns the live policy for one turn, retiring a cached policy whose
    /// facts no longer match the canonical settlement.
    ///
    /// A replacement clears transient view state: the relative age recomputes
    /// on the next reveal, and a stale copy notice must not survive revised
    /// facts.
    fn sync_footer_policy(
        &mut self,
        turn: &TurnId,
        settled_at: String,
        response_text: String,
    ) -> &mut ConversationTurnFooterPolicy {
        let stale = self.footer_policies.get(turn).is_some_and(|policy| {
            footer_policy_is_stale(policy, &settled_at, &response_text)
        });
        if stale {
            self.footer_policies.remove(turn);
        }
        self.footer_policies.entry(turn.clone()).or_insert_with(|| {
            ConversationTurnFooterPolicy::new(settled_at, response_text, String::new())
        })
    }

    /// Serves one footer reveal through the existing footer policy.
    ///
    /// The scene settlement supplies the timestamp and response bytes; the
    /// reveal takes exactly one clock sample, formats it through the existing
    /// relative-age adapter, and mirrors the text into the surface. A reveal
    /// with no settlement is stale paint and drains as a no-op.
    fn route_footer_revealed(
        &mut self,
        turn: TurnId,
        surface: &Entity<ConversationSurface>,
        cx: &mut Context<Self>,
    ) -> SurfaceRouteDecision {
        let staged: Option<(String, String)> =
            settled_footer_for(surface.read(cx).scene(), &turn);
        let Some((settled_at, response_text)) = staged else {
            return SurfaceRouteDecision::Accepted;
        };
        let policy = self.sync_footer_policy(&turn, settled_at.clone(), response_text);
        if policy.observe(TurnFooterInput::Hover) == TurnFooterAction::RequestClockSample {
            let age = format_relative_age(host_now_millis(), &settled_at);
            policy.set_relative_age(age.clone());
            surface.update(cx, |surface, surface_cx| {
                surface.set_footer_relative_age(&turn, age, surface_cx);
            });
        }
        SurfaceRouteDecision::Accepted
    }

    /// Serves one footer copy through the existing footer policy and the
    /// platform clipboard, mirroring the actual outcome into the surface.
    ///
    /// The platform write is a void API with no failure signal, so success is
    /// settled unconditionally and honestly: no speculative failure path is
    /// claimed. The scene settlement supplies the bytes, never the action.
    fn route_footer_copy(
        &mut self,
        turn: TurnId,
        surface: &Entity<ConversationSurface>,
        cx: &mut Context<Self>,
    ) -> SurfaceRouteDecision {
        let staged: Option<(String, String)> =
            settled_footer_for(surface.read(cx).scene(), &turn);
        let Some((settled_at, response_text)) = staged else {
            return SurfaceRouteDecision::Accepted;
        };
        let policy = self.sync_footer_policy(&turn, settled_at, response_text);
        if let TurnFooterAction::CopyResponse { text } = policy.start_copy() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            policy.settle_copy(CopyOutcome::Succeeded);
        }
        let message = policy.copy_message().to_owned();
        surface.update(cx, |surface, surface_cx| {
            surface.set_footer_copy_message(&turn, message, surface_cx);
        });
        SurfaceRouteDecision::Accepted
    }

    fn controller_effects_invalidate_render(&self, before: usize) -> bool {
        self.controller
            .pending_effects()
            .get(before..)
            .is_some_and(|effects| effects.iter().any(effect_invalidates_render))
    }

    fn controller_effects_require_extent_change(&self, before: usize) -> bool {
        self.controller
            .pending_effects()
            .get(before..)
            .is_some_and(|effects| effects.iter().any(effect_requires_extent_change))
    }

    fn dispatch_pending_extent_changes(&mut self) -> Result<(), ConversationStateError> {
        while self.pending_extent_changes > 0 {
            match self.controller.dispatch(ConversationStateEvent::Viewport(
                ViewportEvent::ExtentChanged,
            )) {
                Ok(()) => self.pending_extent_changes -= 1,
                Err(error) => {
                    if !matches!(&error, ConversationStateError::CapacityExhausted { .. }) {
                        self.pending_extent_changes -= 1;
                    }
                    return Err(error);
                }
            }
        }
        Ok(())
    }

    fn retain_refusal(&mut self, error: ConversationStateError) -> bool {
        self.flush_controller_effects();
        if self.controller.pending_effect_count() > 0
            || self.effects.len() >= CONVERSATION_HOST_MAX_EFFECTS
        {
            return false;
        }
        self.effects.push(ConversationHostEffect::Refused {
            refusal: ConversationHostRefusal::Controller(error),
        });
        true
    }

    fn flush_controller_effects(&mut self) {
        let pending = self.controller.pending_effect_count();
        let available = CONVERSATION_HOST_MAX_EFFECTS.saturating_sub(self.effects.len());
        if pending == 0 || pending > available {
            return;
        }
        self.effects.extend(
            self.controller
                .drain_effects()
                .into_iter()
                .map(ConversationHostEffect::Controller),
        );
    }
}

fn effect_invalidates_render(effect: &ConversationStateEffect) -> bool {
    matches!(
        effect,
        ConversationStateEffect::SceneInvalidated
            | ConversationStateEffect::Delivery(ConversationDeliveryEffect::Invalidate)
            | ConversationStateEffect::Steering {
                effect: SteeringEffect::RenderInvalidation { .. },
                ..
            }
            | ConversationStateEffect::Viewport(ViewportEffect::InvalidateRender)
    )
}

fn effect_requires_extent_change(effect: &ConversationStateEffect) -> bool {
    matches!(
        effect,
        ConversationStateEffect::SceneInvalidated
            | ConversationStateEffect::Delivery(ConversationDeliveryEffect::Invalidate)
            | ConversationStateEffect::Steering {
                effect: SteeringEffect::RenderInvalidation { .. },
                ..
            }
    )
}

impl Render for ConversationHost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.surface.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        footer_policy_is_stale, host_now_millis, iso_millis, scene_has_active_work,
        settled_footer_for,
    };
    use crate::conversation_relative_age::format_relative_age;
    use crate::conversation_scene::{
        ConversationScene, SceneTurn, TurnFooterSettlement, TurnNarration, TurnNarrationEntry,
    };
    use crate::conversation_turn_footer_policy::{
        ConversationTurnFooterPolicy, CopyOutcome, TurnFooterAction, TurnFooterInput,
    };
    use artisan_domain::{ConversationLifecycle, TurnId};

    fn turn_id(value: &str) -> TurnId {
        TurnId::parse(value).expect("turn id is valid")
    }

    fn scene_with_narration(narration: TurnNarration) -> ConversationScene {
        ConversationScene::build(
            vec![SceneTurn::new(
                turn_id("turn_a"),
                0,
                ConversationLifecycle::Active,
            )],
            Vec::new(),
            vec![TurnNarrationEntry::new(turn_id("turn_a"), narration)],
            Vec::new(),
        )
        .expect("conversation scene is valid")
    }

    #[test]
    fn settled_iso_vectors_share_the_domain_formatter() {
        assert_eq!(iso_millis(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_millis(1), "1970-01-01T00:00:00.001Z");
        assert_eq!(iso_millis(1_000), "1970-01-01T00:00:01Z");
        assert_eq!(iso_millis(60_000), "1970-01-01T00:01:00Z");
        assert_eq!(iso_millis(3_600_000), "1970-01-01T01:00:00Z");
        assert_eq!(iso_millis(86_400_000), "1970-01-02T00:00:00Z");
        assert_eq!(iso_millis(1_704_067_200_000), "2024-01-01T00:00:00Z");
        assert_eq!(iso_millis(1_709_164_800_000), "2024-02-29T00:00:00Z");
        assert_eq!(iso_millis(1_709_164_800_123), "2024-02-29T00:00:00.123Z");
    }

    #[test]
    fn relative_age_accepts_settled_iso() {
        assert_eq!(
            format_relative_age(1_704_067_200_000 + 90_000, "2024-01-01T00:00:00.000Z"),
            "1m ago"
        );
        assert_eq!(
            format_relative_age(1_704_067_200_000, "2024-01-01T00:00:00.000Z"),
            "0s ago"
        );
    }

    #[test]
    fn host_clock_is_sane_and_nondecreasing() {
        let first = host_now_millis();
        assert!(
            first > 1_700_000_000_000,
            "host clock must read current wall time"
        );
        assert!(host_now_millis() >= first);
    }

    #[test]
    fn scene_active_work_reflects_narration() {
        assert!(scene_has_active_work(&scene_with_narration(
            TurnNarration::Working
        )));
        assert!(scene_has_active_work(&scene_with_narration(
            TurnNarration::Thinking
        )));
        assert!(!scene_has_active_work(&scene_with_narration(
            TurnNarration::Quiet
        )));
        assert!(!scene_has_active_work(&scene_with_narration(
            TurnNarration::WorkedFor { millis: 1_000 }
        )));
        assert!(!scene_has_active_work(&scene_with_narration(
            TurnNarration::Failed
        )));
    }

    #[test]
    fn settled_footer_absent_without_settlement() {
        let scene = scene_with_narration(TurnNarration::WorkedFor { millis: 1_000 });
        assert!(settled_footer_for(&scene, &turn_id("turn_a")).is_none());
        assert!(settled_footer_for(&scene, &turn_id("turn_missing")).is_none());
    }

    #[test]
    fn unknown_turn_rejects_footer_settlement() {
        let mut scene = scene_with_narration(TurnNarration::WorkedFor { millis: 1_000 });
        let settlement =
            TurnFooterSettlement::new("hello".to_owned(), 1_704_067_200_000).expect("bounded");
        assert!(!scene.set_turn_footer_settlement(&turn_id("turn_missing"), settlement));
    }

    #[test]
    fn settled_footer_returns_exact_facts() {
        let mut scene = scene_with_narration(TurnNarration::WorkedFor { millis: 1_000 });
        let settlement =
            TurnFooterSettlement::new("hello".to_owned(), 1_704_067_200_000).expect("bounded");
        assert!(scene.set_turn_footer_settlement(&turn_id("turn_a"), settlement));
        assert_eq!(
            settled_footer_for(&scene, &turn_id("turn_a")),
            Some(("2024-01-01T00:00:00Z".to_owned(), "hello".to_owned()))
        );
    }

    #[test]
    fn stale_policy_facts_retire_on_revision() {
        let policy = ConversationTurnFooterPolicy::new(
            "2024-01-01T00:00:00Z",
            "hello",
            String::new(),
        );
        assert!(!footer_policy_is_stale(
            &policy,
            "2024-01-01T00:00:00Z",
            "hello"
        ));
        assert!(footer_policy_is_stale(
            &policy,
            "2024-01-01T00:00:00Z",
            "revised reply"
        ));
        assert!(footer_policy_is_stale(
            &policy,
            "2024-01-01T00:01:00Z",
            "hello"
        ));
    }

    #[test]
    fn footer_policy_reveal_and_copy_flow() {
        let mut policy = ConversationTurnFooterPolicy::new(
            "2024-01-01T00:00:00.000Z",
            "hello",
            String::new(),
        );
        assert_eq!(
            policy.observe(TurnFooterInput::Hover),
            TurnFooterAction::RequestClockSample
        );
        assert_eq!(
            policy.observe(TurnFooterInput::Focus),
            TurnFooterAction::RequestClockSample
        );
        policy.set_relative_age("1m ago");
        assert_eq!(policy.relative_age(), "1m ago");
        assert_eq!(policy.response_text(), "hello");
        let TurnFooterAction::CopyResponse { text } = policy.start_copy() else {
            panic!("copy input must start the exact copy command");
        };
        assert_eq!(text, "hello");
        policy.settle_copy(CopyOutcome::Succeeded);
        assert_eq!(policy.copy_message(), "");
        policy.settle_copy(CopyOutcome::Failed);
        assert!(!policy.copy_message().is_empty());
    }

    #[test]
    fn footer_policy_rejects_timer_wakeups() {
        let mut policy =
            ConversationTurnFooterPolicy::new("2024-01-01T00:00:00.000Z", "hello", String::new());
        assert_eq!(
            policy.observe(TurnFooterInput::PeriodicTick),
            TurnFooterAction::NoOp
        );
    }
}
