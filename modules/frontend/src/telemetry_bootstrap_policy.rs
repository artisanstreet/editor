//! Pure renderer-start telemetry admission and lifecycle state.
//!
//! This is the dependency-free native counterpart of
//! `modules/frontend/src/lib/telemetry/product-bootstrap.ts`. The caller
//! supplies an already-classified runtime surface, renderer-start timestamp,
//! ready timestamp, and observed lifecycle signals. This module returns a
//! typed capture action and keeps the one-shot admission state; it does not
//! call a telemetry controller, inspect a clock or browser global, schedule a
//! retry, or report a capture error.

#![allow(clippy::module_name_repetitions)]

pub use crate::runtime_surface::{
    EditorSessionStartedTelemetryIntent, MAX_TIME_TO_READY_MS, RuntimeSurface,
};

use crate::runtime_surface::time_to_ready_ms;

/// The exact event name sent to the telemetry controller.
pub const EDITOR_SESSION_STARTED_EVENT: &str = "editor_session_started";

/// Identifies the in-memory renderer lifecycle owned by one policy.
///
/// The identifier scopes admission state only. It is never included in the
/// telemetry intent, where raw lifecycle identifiers are not permitted.
#[must_use]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RendererLifecycleId(u64);

impl RendererLifecycleId {
    /// Creates an opaque lifecycle identifier from a caller-owned value.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the caller-owned value represented by this identifier.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl From<u64> for RendererLifecycleId {
    fn from(value: u64) -> Self {
        Self::new(value)
    }
}

/// A renderer signal relevant to startup-capture admission.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RendererSignal {
    /// The renderer reached ready at the supplied millisecond timestamp.
    Ready { ready_at_ms: i64 },
    /// The connection controller observed a retry signal.
    ConnectionRetry,
}

/// The result observed after the caller executes an admitted capture action.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CaptureOutcome {
    /// The capture operation completed successfully.
    Succeeded,
    /// The capture operation failed; the failure is intentionally absorbed.
    Failed,
}

/// State of one renderer lifecycle's startup-capture admission.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TelemetryBootstrapState {
    /// No ready signal has admitted the startup capture yet.
    AwaitingReady,
    /// The one startup capture action was emitted and awaits its result.
    CaptureAdmitted,
    /// The startup capture completed successfully.
    Captured,
    /// The startup capture failed, and the failure was absorbed permanently
    /// for this renderer lifecycle.
    CaptureFailureAbsorbed,
}

impl TelemetryBootstrapState {
    /// Returns whether the renderer has reached readiness from this state.
    ///
    /// A capture failure does not become a readiness failure: the renderer is
    /// still ready after the capture action was admitted.
    #[must_use]
    pub const fn is_ready(self) -> bool {
        !matches!(self, Self::AwaitingReady)
    }

    /// Returns whether the one-shot capture admission is already closed.
    #[must_use]
    pub const fn capture_is_terminal(self) -> bool {
        matches!(self, Self::Captured | Self::CaptureFailureAbsorbed)
    }
}

/// One pure action emitted by a bootstrap state transition.
#[must_use]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TelemetryBootstrapAction {
    /// No telemetry operation is admitted for this signal.
    NoOp,
    /// Invoke the existing telemetry controller with this typed intent.
    Capture(EditorSessionStartedTelemetryIntent),
}

impl TelemetryBootstrapAction {
    /// Returns the intent when this action admits a capture.
    #[must_use]
    pub const fn capture_intent(self) -> Option<EditorSessionStartedTelemetryIntent> {
        match self {
            Self::NoOp => None,
            Self::Capture(intent) => Some(intent),
        }
    }

    /// Returns whether this action admits the one startup capture.
    #[must_use]
    pub const fn is_capture(self) -> bool {
        matches!(self, Self::Capture(_))
    }
}

/// The action and state after one observed signal or capture result.
#[must_use]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TelemetryBootstrapTransition {
    /// State after applying the observation.
    pub state: TelemetryBootstrapState,
    /// The one action, if this observation admitted it.
    pub action: TelemetryBootstrapAction,
}

impl TelemetryBootstrapTransition {
    /// Returns the resulting state.
    #[must_use]
    pub const fn state(self) -> TelemetryBootstrapState {
        self.state
    }

    /// Returns the resulting action.
    pub const fn action(self) -> TelemetryBootstrapAction {
        self.action
    }
}

/// Builds the exact typed startup event for an already-classified surface.
///
/// The two timestamps are observations supplied by the host. The difference
/// is clamped to the inclusive `0..=MAX_TIME_TO_READY_MS` range, including for
/// signed extremes; no clock is read here.
#[must_use]
pub fn editor_session_started_intent(
    surface: RuntimeSurface,
    renderer_started_at_ms: i64,
    ready_at_ms: i64,
) -> EditorSessionStartedTelemetryIntent {
    EditorSessionStartedTelemetryIntent {
        event: EDITOR_SESSION_STARTED_EVENT,
        forge_connection: surface.forge_connection(),
        surface: surface.telemetry_surface(),
        time_to_ready_ms: time_to_ready_ms(renderer_started_at_ms, ready_at_ms),
    }
}

/// One-shot startup telemetry admission for one renderer lifecycle.
///
/// A policy owns exactly one lifecycle identity and its in-memory state. The
/// first `Ready` signal emits one capture action. Duplicate `Ready` signals,
/// `ConnectionRetry` signals, and any later capture result cannot emit a
/// second action. A caller that executes the action should report either
/// [`CaptureOutcome::Succeeded`] or [`CaptureOutcome::Failed`] through
/// [`Self::settle_capture`]; both outcomes close admission, and the latter is
/// represented as an absorbed state rather than an error.
#[must_use]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TelemetryBootstrapPolicy {
    lifecycle_id: RendererLifecycleId,
    renderer_started_at_ms: i64,
    surface: RuntimeSurface,
    state: TelemetryBootstrapState,
}

impl TelemetryBootstrapPolicy {
    /// Creates fresh startup-capture state for one renderer lifecycle.
    pub fn new(
        lifecycle_id: impl Into<RendererLifecycleId>,
        renderer_started_at_ms: i64,
        surface: RuntimeSurface,
    ) -> Self {
        Self {
            lifecycle_id: lifecycle_id.into(),
            renderer_started_at_ms,
            surface,
            state: TelemetryBootstrapState::AwaitingReady,
        }
    }

    /// Returns the identity that scopes this policy's admission state.
    pub const fn lifecycle_id(self) -> RendererLifecycleId {
        self.lifecycle_id
    }

    /// Returns the supplied renderer-start timestamp.
    #[must_use]
    pub const fn renderer_started_at_ms(self) -> i64 {
        self.renderer_started_at_ms
    }

    /// Returns the already-classified runtime surface.
    #[must_use]
    pub const fn surface(self) -> RuntimeSurface {
        self.surface
    }

    /// Returns the current admission state.
    #[must_use]
    pub const fn state(self) -> TelemetryBootstrapState {
        self.state
    }

    /// Applies one renderer signal and returns its typed action/state result.
    ///
    /// Only the first `Ready` signal while awaiting readiness admits a
    /// capture. A connection retry is never a startup-capture trigger. Once
    /// admission has happened, all duplicate or later signals return
    /// [`TelemetryBootstrapAction::NoOp`].
    pub fn observe(&mut self, signal: RendererSignal) -> TelemetryBootstrapTransition {
        let action = match (self.state, signal) {
            (TelemetryBootstrapState::AwaitingReady, RendererSignal::Ready { ready_at_ms }) => {
                self.state = TelemetryBootstrapState::CaptureAdmitted;
                TelemetryBootstrapAction::Capture(editor_session_started_intent(
                    self.surface,
                    self.renderer_started_at_ms,
                    ready_at_ms,
                ))
            }
            _ => TelemetryBootstrapAction::NoOp,
        };

        TelemetryBootstrapTransition {
            state: self.state,
            action,
        }
    }

    /// Applies the result of the one admitted capture without propagating it.
    ///
    /// A result received before admission or after a terminal result is a
    /// no-op. In particular, `Failed` closes the capture admission and does
    /// not make readiness fail or authorize a retry.
    pub fn settle_capture(&mut self, outcome: CaptureOutcome) -> TelemetryBootstrapTransition {
        if self.state == TelemetryBootstrapState::CaptureAdmitted {
            self.state = match outcome {
                CaptureOutcome::Succeeded => TelemetryBootstrapState::Captured,
                CaptureOutcome::Failed => TelemetryBootstrapState::CaptureFailureAbsorbed,
            };
        }

        TelemetryBootstrapTransition {
            state: self.state,
            action: TelemetryBootstrapAction::NoOp,
        }
    }

    /// Converts any controller result into the same non-retrying transition.
    ///
    /// The error value is deliberately discarded at this boundary, matching
    /// the source controller's `catchCause(() => Effect.void)` behavior.
    pub fn settle_capture_result<E>(
        &mut self,
        result: &Result<(), E>,
    ) -> TelemetryBootstrapTransition {
        self.settle_capture(if result.is_ok() {
            CaptureOutcome::Succeeded
        } else {
            CaptureOutcome::Failed
        })
    }
}
