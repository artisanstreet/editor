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

use artisan_domain::{IdentifierError, ThreadId};
use artisan_ui::theme::ThemeMode;
use gpui::{App, AppContext as _, Context, Entity, IntoElement, Render, Subscription, Window};
use thiserror::Error;

use crate::conversation_delivery_machine::ConversationDeliveryEffect;
use crate::conversation_scene::ConversationScene;
use crate::conversation_state_machine::{
    ConversationStateController, ConversationStateEffect, ConversationStateError,
    ConversationStateEvent, ConversationStateView, MAX_PENDING_EFFECTS,
};
use crate::conversation_steering_machine::SteeringEffect;
use crate::conversation_surface::{
    ConversationSurface, ConversationSurfaceAction, ConversationSurfaceTarget,
};
use crate::conversation_view_machine::ViewportEffect;

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

/// The native host for one fixed conversation thread.
pub struct ConversationHost {
    controller: ConversationStateController,
    surface: Entity<ConversationSurface>,
    /// Kept for the complete host lifetime so surface notifications continue
    /// to route through the controller boundary.
    _surface_subscription: Subscription,
    effects: Vec<ConversationHostEffect>,
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
            host.route_surface_actions(surface, cx);
        });
        let mut host = Self {
            controller,
            surface,
            _surface_subscription: surface_subscription,
            effects: Vec::with_capacity(CONVERSATION_HOST_MAX_EFFECTS),
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
        effects
    }

    /// Retries the oldest surface action after an outer adapter has drained
    /// host effects.
    ///
    /// The stored surface subscription handles ordinary notifications. This
    /// explicit retry is the bounded backpressure seam for an adapter that
    /// drained effects without causing a new surface notification.
    pub fn process_pending_actions(&mut self, cx: &mut Context<Self>) {
        let surface = self.surface.clone();
        self.route_surface_actions(surface, cx);
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
        let before = self.controller.pending_effect_count();
        match self.controller.dispatch(event) {
            Ok(()) => {
                if self.controller_effects_invalidate_render(before) {
                    let scene = match self.controller.scene() {
                        Ok(scene) => scene,
                        Err(error) => {
                            self.flush_controller_effects();
                            cx.notify();
                            return Err(ConversationHostError::SceneProjection(error));
                        }
                    };
                    self.surface.update(cx, |surface, surface_cx| {
                        surface.replace_scene(scene, surface_cx);
                    });
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
        surface: Entity<ConversationSurface>,
        cx: &mut Context<Self>,
    ) {
        self.flush_controller_effects();
        loop {
            let Some(action) = surface.read(cx).next_action().cloned() else {
                break;
            };
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
                        ConversationStateEvent::Viewport(
                            crate::conversation_view_machine::ViewportEvent::UserScrolled {
                                at_bottom: observation.at_bottom,
                            },
                        ),
                        cx,
                    ),
                ConversationSurfaceAction::ScrollIntent { target } => {
                    self.route_scroll_intent(target, cx)
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
        self.flush_controller_effects();
    }

    fn route_controller_event(
        &mut self,
        event: ConversationStateEvent,
        cx: &mut Context<Self>,
    ) -> SurfaceRouteDecision {
        match self.dispatch(event, cx) {
            Ok(()) | Err(ConversationHostError::Controller(_)) => SurfaceRouteDecision::Accepted,
            Err(ConversationHostError::EffectOutboxFull { .. }) => {
                SurfaceRouteDecision::Backpressured
            }
            Err(ConversationHostError::SceneProjection(_)) => SurfaceRouteDecision::Accepted,
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

    fn controller_effects_invalidate_render(&self, before: usize) -> bool {
        self.controller
            .pending_effects()
            .get(before..)
            .is_some_and(|effects| effects.iter().any(effect_invalidates_render))
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
    match effect {
        ConversationStateEffect::SceneInvalidated => true,
        ConversationStateEffect::Delivery(ConversationDeliveryEffect::Invalidate) => true,
        ConversationStateEffect::Steering {
            effect: SteeringEffect::RenderInvalidation { .. },
            ..
        }
        | ConversationStateEffect::Viewport(ViewportEffect::InvalidateRender) => true,
        _ => false,
    }
}

impl Render for ConversationHost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.surface.clone()
    }
}
