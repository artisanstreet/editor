//! Pure synchronous per-turn authority for the conversation renderer.
//!
//! This module owns one legal state chart from which every live and terminal
//! work label is derived. It replaces scattered `thinking`/`compacting`/
//! `streaming` booleans with a single hierarchical chart. An outer aggregate
//! keeps one [`ConversationTurnController`] per `TurnId` and supplies explicit
//! timestamps and events. Durable snapshot replay, steering, viewport,
//! networking, timers, and GPUI remain outside this boundary.

#![allow(clippy::module_name_repetitions)]

use statig::Outcome::{Handled, Transition};
use statig::blocking::{IntoStateMachineExt, StateMachine};
use statig::prelude::*;

/// How the provider's failure is redacted for the UI.
///
/// Arbitrary provider errors, prompts, command text, raw tool output, and file
/// contents are never stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FailureKind {
    /// Generic or unclassified failure.
    Generic,
    /// Provider timeout.
    Timeout,
    /// Rate limiting.
    RateLimited,
    /// Context window exhausted.
    ContextOverflow,
}

/// Leaf state of the hierarchical chart.
///
/// Shared superstates are explicit in the generated Statig hierarchy:
/// * Active-work superstate: `WaitingForProvider`, `Compacting`, `Thinking`,
///   `Working`, `StreamingReply`, `WaitingForBackground`
/// * Settled superstate: `Completed`, `Failed`, `Interrupted`, `Cancelled`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StateKind {
    Pending,
    WaitingForProvider,
    Compacting,
    Thinking,
    Working,
    StreamingReply,
    WaitingForBackground,
    Completed,
    Failed,
    Interrupted,
    Cancelled,
}

impl StateKind {
    /// Whether this leaf belongs to the active-work superstate.
    #[must_use]
    pub const fn is_active(self) -> bool {
        matches!(
            self,
            Self::WaitingForProvider
                | Self::Compacting
                | Self::Thinking
                | Self::Working
                | Self::StreamingReply
                | Self::WaitingForBackground
        )
    }

    /// Whether this leaf belongs to the settled superstate.
    #[must_use]
    pub const fn is_settled(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Interrupted | Self::Cancelled
        )
    }

    /// Whether this leaf is sealed against different later work/terminal events.
    ///
    /// `Interrupted` is resumable; `Completed`, `Failed`, and `Cancelled` are
    /// sealed.
    #[must_use]
    pub const fn is_sealed(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

/// Which successful-completion copy is correct for the terminal view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompletionKind {
    /// Provider did observable non-reasoning work.
    Worked,
    /// Provider only reasoned.
    Thought,
}

/// Closed narration enum: exactly one label is representable at a time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnNarration {
    /// No visible work label (pending/quiet).
    Hidden,
    /// Waiting for the provider/engine to respond.
    WaitingForProvider,
    /// Exact copy required by the spec.
    Compacting,
    /// Provider reasoning is visible but no tool work yet.
    Thinking,
    /// Ordinary tool/activity work is visible.
    Working,
    /// Assistant reply text is streaming; quiet thinking/working is suppressed.
    StreamingReply,
    /// Provider responded but background agents remain.
    WaitingForBackground,
    /// Terminal success: `Worked for` with elapsed milliseconds.
    WorkedFor { elapsed_ms: u64 },
    /// Terminal success: `Thought for` with elapsed milliseconds.
    ThoughtFor { elapsed_ms: u64 },
    /// Terminal failure.
    Failed {
        elapsed_ms: u64,
        kind: Option<FailureKind>,
    },
    /// Terminal interruption (stopped).
    Interrupted { elapsed_ms: u64 },
    /// Terminal cancellation.
    Cancelled { elapsed_ms: u64 },
}

impl TurnNarration {
    /// Returns the exact renderer copy for this narration.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Hidden => String::new(),
            Self::WaitingForProvider => "Waiting for provider…".to_owned(),
            Self::Compacting => "Compacting the conversation…".to_owned(),
            Self::Thinking => "Thinking…".to_owned(),
            Self::Working => "Working…".to_owned(),
            Self::StreamingReply => String::new(),
            Self::WaitingForBackground => "Waiting for background agents…".to_owned(),
            Self::WorkedFor { .. } => "Worked for".to_owned(),
            Self::ThoughtFor { .. } => "Thought for".to_owned(),
            Self::Failed { .. } => "Failed".to_owned(),
            Self::Interrupted { .. } => "Stopped".to_owned(),
            Self::Cancelled { .. } => "Cancelled".to_owned(),
        }
    }

    /// Whether this narration is a terminal outcome.
    #[must_use]
    pub const fn is_terminal(self: &Self) -> bool {
        matches!(
            self,
            Self::WorkedFor { .. }
                | Self::ThoughtFor { .. }
                | Self::Failed { .. }
                | Self::Interrupted { .. }
                | Self::Cancelled { .. }
        )
    }
}

/// Immutable renderer-facing view derived from the authoritative state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnView {
    /// Exact leaf state kind.
    pub state: StateKind,
    /// Monotonic local revision observed by the controller.
    pub revision: u64,
    /// Turn start instant (first active entry) if any, as signed Unix ms.
    pub started_at: Option<i64>,
    /// Most recent phase start instant if any.
    pub phase_started_at: Option<i64>,
    /// Terminal instant if settled, otherwise `None`.
    pub terminal_at: Option<i64>,
    /// Closed narration: exactly one label.
    pub narration: TurnNarration,
}

/// Closed typed event vocabulary. Every variant needing time carries a
/// caller-supplied signed Unix millisecond timestamp and a monotonic revision.
/// This module never reads a clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnEvent {
    WaitingForProvider {
        at: i64,
        revision: u64,
    },
    Compacting {
        at: i64,
        revision: u64,
    },
    Thinking {
        at: i64,
        revision: u64,
    },
    Working {
        at: i64,
        revision: u64,
    },
    StreamingReply {
        at: i64,
        revision: u64,
    },
    WaitingForBackground {
        at: i64,
        revision: u64,
    },
    Completed {
        at: i64,
        revision: u64,
    },
    Failed {
        at: i64,
        revision: u64,
        kind: Option<FailureKind>,
    },
    Interrupted {
        at: i64,
        revision: u64,
    },
    Cancelled {
        at: i64,
        revision: u64,
    },
    Resume {
        at: i64,
        revision: u64,
    },
}

impl TurnEvent {
    fn at(&self) -> i64 {
        match self {
            Self::WaitingForProvider { at, .. }
            | Self::Compacting { at, .. }
            | Self::Thinking { at, .. }
            | Self::Working { at, .. }
            | Self::StreamingReply { at, .. }
            | Self::WaitingForBackground { at, .. }
            | Self::Completed { at, .. }
            | Self::Failed { at, .. }
            | Self::Interrupted { at, .. }
            | Self::Cancelled { at, .. }
            | Self::Resume { at, .. } => *at,
        }
    }

    fn revision(&self) -> u64 {
        match self {
            Self::WaitingForProvider { revision, .. }
            | Self::Compacting { revision, .. }
            | Self::Thinking { revision, .. }
            | Self::Working { revision, .. }
            | Self::StreamingReply { revision, .. }
            | Self::WaitingForBackground { revision, .. }
            | Self::Completed { revision, .. }
            | Self::Failed { revision, .. }
            | Self::Interrupted { revision, .. }
            | Self::Cancelled { revision, .. }
            | Self::Resume { revision, .. } => *revision,
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Self::WaitingForProvider { .. } => "WaitingForProvider",
            Self::Compacting { .. } => "Compacting",
            Self::Thinking { .. } => "Thinking",
            Self::Working { .. } => "Working",
            Self::StreamingReply { .. } => "StreamingReply",
            Self::WaitingForBackground { .. } => "WaitingForBackground",
            Self::Completed { .. } => "Completed",
            Self::Failed { .. } => "Failed",
            Self::Interrupted { .. } => "Interrupted",
            Self::Cancelled { .. } => "Cancelled",
            Self::Resume { .. } => "Resume",
        }
    }
}

/// Ordered typed effects emitted by the controller, if any.
///
/// The current chart is effect-free for the renderer: all facts are derived
/// from the immutable view. The type remains so callers can match exhaustively
/// without special-casing `()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnEffect {}

/// Typed refusal reasons. The prior state/view is left unchanged on error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnError {
    TimestampRegression {
        expected_at_least: i64,
        got: i64,
    },
    StaleRevision {
        expected_at_least: u64,
        got: u64,
    },
    Sealed {
        state: StateKind,
        event: &'static str,
    },
}

impl std::fmt::Display for TurnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TimestampRegression {
                expected_at_least,
                got,
            } => write!(
                f,
                "timestamp regression: expected at least {expected_at_least}, got {got}"
            ),
            Self::StaleRevision {
                expected_at_least,
                got,
            } => write!(
                f,
                "stale revision: expected at least {expected_at_least}, got {got}"
            ),
            Self::Sealed { state, event } => {
                write!(f, "sealed state {state:?} rejects event {event}")
            }
        }
    }
}

impl std::error::Error for TurnError {}

fn saturating_elapsed_ms(start: i64, end: i64) -> u64 {
    let start_128 = i128::from(start);
    let end_128 = i128::from(end);
    let diff = end_128 - start_128;
    if diff <= 0 {
        0
    } else {
        let diff_u128 = diff as u128;
        if diff_u128 > u128::from(u64::MAX) {
            u64::MAX
        } else {
            diff_u128 as u64
        }
    }
}

// ---------------------------------------------------------------------------
// Statig 0.4.1 blocking state machine — shared storage and hierarchy.
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct TurnMachine {
    started_at: Option<i64>,
    phase_started_at: Option<i64>,
    last_timestamp: Option<i64>,
    revision: u64,
    reasoning_seen: bool,
    work_seen: bool,
    terminal_at: Option<i64>,
    failure_kind: Option<FailureKind>,
}

impl TurnMachine {
    fn apply_active(&mut self, at: i64, revision: u64) {
        if self.started_at.is_none() {
            self.started_at = Some(at);
        }
        self.phase_started_at = Some(at);
        self.last_timestamp = Some(at);
        if revision > self.revision {
            self.revision = revision;
        }
    }

    fn apply_terminal(&mut self, at: i64, revision: u64, failure_kind: Option<FailureKind>) {
        if self.started_at.is_none() {
            self.started_at = Some(at);
            self.phase_started_at = Some(at);
        } else {
            self.phase_started_at = Some(at);
        }
        self.last_timestamp = Some(at);
        if revision > self.revision {
            self.revision = revision;
        }
        self.terminal_at = Some(at);
        self.failure_kind = failure_kind;
    }

    fn clear_terminal_for_resume(&mut self) {
        self.terminal_at = None;
        self.failure_kind = None;
    }

    fn completion_kind(&self) -> CompletionKind {
        if self.work_seen {
            CompletionKind::Worked
        } else {
            CompletionKind::Thought
        }
    }
}

#[statig::state_machine(
    initial = "State::pending()",
    state(derive(Debug, Clone, PartialEq, Eq)),
    superstate(derive(Debug, Clone, PartialEq, Eq))
)]
impl TurnMachine {
    #[state]
    fn pending(&mut self, event: &TurnEvent) -> Outcome<State> {
        match event {
            TurnEvent::WaitingForProvider { at, revision } => {
                self.apply_active(*at, *revision);
                Transition(State::waiting_for_provider())
            }
            TurnEvent::Compacting { at, revision } => {
                self.apply_active(*at, *revision);
                Transition(State::compacting())
            }
            TurnEvent::Thinking { at, revision } => {
                self.apply_active(*at, *revision);
                self.reasoning_seen = true;
                Transition(State::thinking())
            }
            TurnEvent::Working { at, revision } => {
                self.apply_active(*at, *revision);
                self.work_seen = true;
                Transition(State::working())
            }
            TurnEvent::StreamingReply { at, revision } => {
                self.apply_active(*at, *revision);
                Transition(State::streaming_reply())
            }
            TurnEvent::WaitingForBackground { at, revision } => {
                self.apply_active(*at, *revision);
                Transition(State::waiting_for_background())
            }
            TurnEvent::Completed { at, revision } => {
                self.apply_terminal(*at, *revision, None);
                Transition(State::completed())
            }
            TurnEvent::Failed { at, revision, kind } => {
                self.apply_terminal(*at, *revision, *kind);
                Transition(State::failed())
            }
            TurnEvent::Interrupted { at, revision } => {
                self.apply_terminal(*at, *revision, None);
                Transition(State::interrupted())
            }
            TurnEvent::Cancelled { at, revision } => {
                self.apply_terminal(*at, *revision, None);
                Transition(State::cancelled())
            }
            TurnEvent::Resume { .. } => Handled,
        }
    }

    #[state(superstate = "active_work")]
    fn waiting_for_provider(&mut self, event: &TurnEvent) -> Outcome<State> {
        match event {
            TurnEvent::WaitingForProvider { at, revision } => {
                self.apply_active(*at, *revision);
                Transition(State::waiting_for_provider())
            }
            TurnEvent::Compacting { at, revision } => {
                self.apply_active(*at, *revision);
                Transition(State::compacting())
            }
            TurnEvent::Thinking { at, revision } => {
                self.apply_active(*at, *revision);
                self.reasoning_seen = true;
                Transition(State::thinking())
            }
            TurnEvent::Working { at, revision } => {
                self.apply_active(*at, *revision);
                self.work_seen = true;
                Transition(State::working())
            }
            TurnEvent::StreamingReply { at, revision } => {
                self.apply_active(*at, *revision);
                Transition(State::streaming_reply())
            }
            TurnEvent::WaitingForBackground { at, revision } => {
                self.apply_active(*at, *revision);
                Transition(State::waiting_for_background())
            }
            TurnEvent::Completed { at, revision } => {
                self.apply_terminal(*at, *revision, None);
                Transition(State::completed())
            }
            TurnEvent::Failed { at, revision, kind } => {
                self.apply_terminal(*at, *revision, *kind);
                Transition(State::failed())
            }
            TurnEvent::Interrupted { at, revision } => {
                self.apply_terminal(*at, *revision, None);
                Transition(State::interrupted())
            }
            TurnEvent::Cancelled { at, revision } => {
                self.apply_terminal(*at, *revision, None);
                Transition(State::cancelled())
            }
            TurnEvent::Resume { .. } => Handled,
        }
    }

    #[state(superstate = "active_work")]
    fn compacting(&mut self, event: &TurnEvent) -> Outcome<State> {
        match event {
            TurnEvent::WaitingForProvider { at, revision } => {
                self.apply_active(*at, *revision);
                Transition(State::waiting_for_provider())
            }
            TurnEvent::Compacting { at, revision } => {
                self.apply_active(*at, *revision);
                Transition(State::compacting())
            }
            TurnEvent::Thinking { at, revision } => {
                self.apply_active(*at, *revision);
                self.reasoning_seen = true;
                Transition(State::thinking())
            }
            TurnEvent::Working { at, revision } => {
                self.apply_active(*at, *revision);
                self.work_seen = true;
                Transition(State::working())
            }
            TurnEvent::StreamingReply { at, revision } => {
                self.apply_active(*at, *revision);
                Transition(State::streaming_reply())
            }
            TurnEvent::WaitingForBackground { at, revision } => {
                self.apply_active(*at, *revision);
                Transition(State::waiting_for_background())
            }
            TurnEvent::Completed { at, revision } => {
                self.apply_terminal(*at, *revision, None);
                Transition(State::completed())
            }
            TurnEvent::Failed { at, revision, kind } => {
                self.apply_terminal(*at, *revision, *kind);
                Transition(State::failed())
            }
            TurnEvent::Interrupted { at, revision } => {
                self.apply_terminal(*at, *revision, None);
                Transition(State::interrupted())
            }
            TurnEvent::Cancelled { at, revision } => {
                self.apply_terminal(*at, *revision, None);
                Transition(State::cancelled())
            }
            TurnEvent::Resume { .. } => Handled,
        }
    }

    #[state(superstate = "active_work")]
    fn thinking(&mut self, event: &TurnEvent) -> Outcome<State> {
        match event {
            TurnEvent::WaitingForProvider { at, revision } => {
                self.apply_active(*at, *revision);
                Transition(State::waiting_for_provider())
            }
            TurnEvent::Compacting { at, revision } => {
                self.apply_active(*at, *revision);
                Transition(State::compacting())
            }
            TurnEvent::Thinking { at, revision } => {
                self.apply_active(*at, *revision);
                self.reasoning_seen = true;
                Transition(State::thinking())
            }
            TurnEvent::Working { at, revision } => {
                self.apply_active(*at, *revision);
                self.work_seen = true;
                Transition(State::working())
            }
            TurnEvent::StreamingReply { at, revision } => {
                self.apply_active(*at, *revision);
                Transition(State::streaming_reply())
            }
            TurnEvent::WaitingForBackground { at, revision } => {
                self.apply_active(*at, *revision);
                Transition(State::waiting_for_background())
            }
            TurnEvent::Completed { at, revision } => {
                self.apply_terminal(*at, *revision, None);
                Transition(State::completed())
            }
            TurnEvent::Failed { at, revision, kind } => {
                self.apply_terminal(*at, *revision, *kind);
                Transition(State::failed())
            }
            TurnEvent::Interrupted { at, revision } => {
                self.apply_terminal(*at, *revision, None);
                Transition(State::interrupted())
            }
            TurnEvent::Cancelled { at, revision } => {
                self.apply_terminal(*at, *revision, None);
                Transition(State::cancelled())
            }
            TurnEvent::Resume { .. } => Handled,
        }
    }

    #[state(superstate = "active_work")]
    fn working(&mut self, event: &TurnEvent) -> Outcome<State> {
        match event {
            TurnEvent::WaitingForProvider { at, revision } => {
                self.apply_active(*at, *revision);
                Transition(State::waiting_for_provider())
            }
            TurnEvent::Compacting { at, revision } => {
                self.apply_active(*at, *revision);
                Transition(State::compacting())
            }
            TurnEvent::Thinking { at, revision } => {
                self.apply_active(*at, *revision);
                self.reasoning_seen = true;
                Transition(State::thinking())
            }
            TurnEvent::Working { at, revision } => {
                self.apply_active(*at, *revision);
                self.work_seen = true;
                Transition(State::working())
            }
            TurnEvent::StreamingReply { at, revision } => {
                self.apply_active(*at, *revision);
                Transition(State::streaming_reply())
            }
            TurnEvent::WaitingForBackground { at, revision } => {
                self.apply_active(*at, *revision);
                Transition(State::waiting_for_background())
            }
            TurnEvent::Completed { at, revision } => {
                self.apply_terminal(*at, *revision, None);
                Transition(State::completed())
            }
            TurnEvent::Failed { at, revision, kind } => {
                self.apply_terminal(*at, *revision, *kind);
                Transition(State::failed())
            }
            TurnEvent::Interrupted { at, revision } => {
                self.apply_terminal(*at, *revision, None);
                Transition(State::interrupted())
            }
            TurnEvent::Cancelled { at, revision } => {
                self.apply_terminal(*at, *revision, None);
                Transition(State::cancelled())
            }
            TurnEvent::Resume { .. } => Handled,
        }
    }

    #[state(superstate = "active_work")]
    fn streaming_reply(&mut self, event: &TurnEvent) -> Outcome<State> {
        match event {
            TurnEvent::WaitingForProvider { at, revision } => {
                self.apply_active(*at, *revision);
                Transition(State::waiting_for_provider())
            }
            TurnEvent::Compacting { at, revision } => {
                self.apply_active(*at, *revision);
                Transition(State::compacting())
            }
            TurnEvent::Thinking { at, revision } => {
                self.apply_active(*at, *revision);
                self.reasoning_seen = true;
                Transition(State::thinking())
            }
            TurnEvent::Working { at, revision } => {
                self.apply_active(*at, *revision);
                self.work_seen = true;
                Transition(State::working())
            }
            TurnEvent::StreamingReply { at, revision } => {
                self.apply_active(*at, *revision);
                Transition(State::streaming_reply())
            }
            TurnEvent::WaitingForBackground { at, revision } => {
                self.apply_active(*at, *revision);
                Transition(State::waiting_for_background())
            }
            TurnEvent::Completed { at, revision } => {
                self.apply_terminal(*at, *revision, None);
                Transition(State::completed())
            }
            TurnEvent::Failed { at, revision, kind } => {
                self.apply_terminal(*at, *revision, *kind);
                Transition(State::failed())
            }
            TurnEvent::Interrupted { at, revision } => {
                self.apply_terminal(*at, *revision, None);
                Transition(State::interrupted())
            }
            TurnEvent::Cancelled { at, revision } => {
                self.apply_terminal(*at, *revision, None);
                Transition(State::cancelled())
            }
            TurnEvent::Resume { .. } => Handled,
        }
    }

    #[state(superstate = "active_work")]
    fn waiting_for_background(&mut self, event: &TurnEvent) -> Outcome<State> {
        match event {
            TurnEvent::WaitingForProvider { at, revision } => {
                self.apply_active(*at, *revision);
                Transition(State::waiting_for_provider())
            }
            TurnEvent::Compacting { at, revision } => {
                self.apply_active(*at, *revision);
                Transition(State::compacting())
            }
            TurnEvent::Thinking { at, revision } => {
                self.apply_active(*at, *revision);
                self.reasoning_seen = true;
                Transition(State::thinking())
            }
            TurnEvent::Working { at, revision } => {
                self.apply_active(*at, *revision);
                self.work_seen = true;
                Transition(State::working())
            }
            TurnEvent::StreamingReply { at, revision } => {
                self.apply_active(*at, *revision);
                Transition(State::streaming_reply())
            }
            TurnEvent::WaitingForBackground { at, revision } => {
                self.apply_active(*at, *revision);
                Transition(State::waiting_for_background())
            }
            TurnEvent::Completed { at, revision } => {
                self.apply_terminal(*at, *revision, None);
                Transition(State::completed())
            }
            TurnEvent::Failed { at, revision, kind } => {
                self.apply_terminal(*at, *revision, *kind);
                Transition(State::failed())
            }
            TurnEvent::Interrupted { at, revision } => {
                self.apply_terminal(*at, *revision, None);
                Transition(State::interrupted())
            }
            TurnEvent::Cancelled { at, revision } => {
                self.apply_terminal(*at, *revision, None);
                Transition(State::cancelled())
            }
            TurnEvent::Resume { .. } => Handled,
        }
    }

    #[state(superstate = "settled")]
    fn completed(&mut self, event: &TurnEvent) -> Outcome<State> {
        match event {
            TurnEvent::Completed { at, revision } => {
                self.apply_terminal(*at, *revision, None);
                Handled
            }
            _ => Handled,
        }
    }

    #[state(superstate = "settled")]
    fn failed(&mut self, event: &TurnEvent) -> Outcome<State> {
        match event {
            TurnEvent::Failed { at, revision, kind } => {
                self.apply_terminal(*at, *revision, *kind);
                Handled
            }
            _ => Handled,
        }
    }

    #[state(superstate = "settled")]
    fn interrupted(&mut self, event: &TurnEvent) -> Outcome<State> {
        match event {
            TurnEvent::Resume { at, revision } => {
                self.clear_terminal_for_resume();
                self.apply_active(*at, *revision);
                self.work_seen = true;
                Transition(State::working())
            }
            TurnEvent::WaitingForProvider { at, revision } => {
                self.clear_terminal_for_resume();
                self.apply_active(*at, *revision);
                Transition(State::waiting_for_provider())
            }
            TurnEvent::Compacting { at, revision } => {
                self.clear_terminal_for_resume();
                self.apply_active(*at, *revision);
                Transition(State::compacting())
            }
            TurnEvent::Thinking { at, revision } => {
                self.clear_terminal_for_resume();
                self.apply_active(*at, *revision);
                self.reasoning_seen = true;
                Transition(State::thinking())
            }
            TurnEvent::Working { at, revision } => {
                self.clear_terminal_for_resume();
                self.apply_active(*at, *revision);
                self.work_seen = true;
                Transition(State::working())
            }
            TurnEvent::StreamingReply { at, revision } => {
                self.clear_terminal_for_resume();
                self.apply_active(*at, *revision);
                Transition(State::streaming_reply())
            }
            TurnEvent::WaitingForBackground { at, revision } => {
                self.clear_terminal_for_resume();
                self.apply_active(*at, *revision);
                Transition(State::waiting_for_background())
            }
            TurnEvent::Completed { at, revision } => {
                self.apply_terminal(*at, *revision, None);
                Transition(State::completed())
            }
            TurnEvent::Failed { at, revision, kind } => {
                self.apply_terminal(*at, *revision, *kind);
                Transition(State::failed())
            }
            TurnEvent::Cancelled { at, revision } => {
                self.apply_terminal(*at, *revision, None);
                Transition(State::cancelled())
            }
            TurnEvent::Interrupted { at, revision } => {
                self.apply_terminal(*at, *revision, None);
                Transition(State::interrupted())
            }
        }
    }

    #[state(superstate = "settled")]
    fn cancelled(&mut self, event: &TurnEvent) -> Outcome<State> {
        match event {
            TurnEvent::Cancelled { at, revision } => {
                self.apply_terminal(*at, *revision, None);
                Handled
            }
            _ => Handled,
        }
    }

    #[superstate]
    fn active_work(&mut self, event: &TurnEvent) -> Outcome<State> {
        let _ = event;
        Handled
    }

    #[superstate]
    fn settled(&mut self, event: &TurnEvent) -> Outcome<State> {
        let _ = event;
        Handled
    }
}

/// Public synchronous controller. One instance per `TurnId` is kept by an
/// outer aggregate.
pub struct ConversationTurnController {
    machine: StateMachine<TurnMachine>,
}

impl Default for ConversationTurnController {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ConversationTurnController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConversationTurnController")
            .field("state", self.machine.state())
            .field("inner", self.machine.inner())
            .finish()
    }
}

impl ConversationTurnController {
    /// Creates a controller in `Pending` at revision zero.
    #[must_use]
    pub fn new() -> Self {
        Self {
            machine: TurnMachine::default().state_machine(),
        }
    }

    /// Returns the current leaf state kind.
    #[must_use]
    pub fn state(&self) -> StateKind {
        match self.machine.state() {
            State::Pending { .. } => StateKind::Pending,
            State::WaitingForProvider { .. } => StateKind::WaitingForProvider,
            State::Compacting { .. } => StateKind::Compacting,
            State::Thinking { .. } => StateKind::Thinking,
            State::Working { .. } => StateKind::Working,
            State::StreamingReply { .. } => StateKind::StreamingReply,
            State::WaitingForBackground { .. } => StateKind::WaitingForBackground,
            State::Completed { .. } => StateKind::Completed,
            State::Failed { .. } => StateKind::Failed,
            State::Interrupted { .. } => StateKind::Interrupted,
            State::Cancelled { .. } => StateKind::Cancelled,
        }
    }

    /// Returns the current monotonic revision.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.machine.inner().revision
    }

    /// Returns the turn start instant, if any.
    #[must_use]
    pub fn started_at(&self) -> Option<i64> {
        self.machine.inner().started_at
    }

    /// Returns the most recent phase start, if any.
    #[must_use]
    pub fn phase_started_at(&self) -> Option<i64> {
        self.machine.inner().phase_started_at
    }

    /// Returns the terminal instant, if settled.
    #[must_use]
    pub fn terminal_at(&self) -> Option<i64> {
        self.machine.inner().terminal_at
    }

    /// Returns whether any reasoning was visible in this turn.
    #[must_use]
    pub fn reasoning_seen(&self) -> bool {
        self.machine.inner().reasoning_seen
    }

    /// Returns whether any non-reasoning work was visible in this turn.
    #[must_use]
    pub fn work_seen(&self) -> bool {
        self.machine.inner().work_seen
    }

    /// Derives the immutable renderer-facing view.
    #[must_use]
    pub fn view(&self) -> TurnView {
        let inner = self.machine.inner();
        let state = self.state();
        let narration = match state {
            StateKind::Pending => TurnNarration::Hidden,
            StateKind::WaitingForProvider => TurnNarration::WaitingForProvider,
            StateKind::Compacting => TurnNarration::Compacting,
            StateKind::Thinking => TurnNarration::Thinking,
            StateKind::Working => TurnNarration::Working,
            StateKind::StreamingReply => TurnNarration::StreamingReply,
            StateKind::WaitingForBackground => TurnNarration::WaitingForBackground,
            StateKind::Completed => {
                let start = inner
                    .started_at
                    .unwrap_or_else(|| inner.terminal_at.unwrap_or(0));
                let end = inner.terminal_at.unwrap_or(start);
                let elapsed_ms = saturating_elapsed_ms(start, end);
                match inner.completion_kind() {
                    CompletionKind::Worked => TurnNarration::WorkedFor { elapsed_ms },
                    CompletionKind::Thought => TurnNarration::ThoughtFor { elapsed_ms },
                }
            }
            StateKind::Failed => {
                let start = inner
                    .started_at
                    .unwrap_or_else(|| inner.terminal_at.unwrap_or(0));
                let end = inner.terminal_at.unwrap_or(start);
                let elapsed_ms = saturating_elapsed_ms(start, end);
                TurnNarration::Failed {
                    elapsed_ms,
                    kind: inner.failure_kind,
                }
            }
            StateKind::Interrupted => {
                let start = inner
                    .started_at
                    .unwrap_or_else(|| inner.terminal_at.unwrap_or(0));
                let end = inner.terminal_at.unwrap_or(start);
                let elapsed_ms = saturating_elapsed_ms(start, end);
                TurnNarration::Interrupted { elapsed_ms }
            }
            StateKind::Cancelled => {
                let start = inner
                    .started_at
                    .unwrap_or_else(|| inner.terminal_at.unwrap_or(0));
                let end = inner.terminal_at.unwrap_or(start);
                let elapsed_ms = saturating_elapsed_ms(start, end);
                TurnNarration::Cancelled { elapsed_ms }
            }
        };

        TurnView {
            state,
            revision: inner.revision,
            started_at: inner.started_at,
            phase_started_at: inner.phase_started_at,
            terminal_at: inner.terminal_at,
            narration,
        }
    }

    /// Dispatches one typed event, enforcing monotonic time/revision and
    /// sealed-state semantics atomically.
    ///
    /// On error the prior state/view is left unchanged. Ordered typed effects
    /// are returned on success (currently none; the view is authoritative).
    pub fn dispatch(&mut self, event: TurnEvent) -> Result<Vec<TurnEffect>, TurnError> {
        let at = event.at();
        let revision = event.revision();
        let event_name = event.name();

        let inner = self.machine.inner();
        if let Some(last) = inner.last_timestamp {
            if at < last {
                return Err(TurnError::TimestampRegression {
                    expected_at_least: last,
                    got: at,
                });
            }
        }
        if revision < inner.revision {
            return Err(TurnError::StaleRevision {
                expected_at_least: inner.revision,
                got: revision,
            });
        }

        let state_kind = self.state();
        if state_kind.is_sealed() {
            let is_duplicate_settlement = match (state_kind, &event) {
                (StateKind::Completed, TurnEvent::Completed { at: ea, .. }) => {
                    inner.terminal_at == Some(*ea)
                }
                (StateKind::Failed, TurnEvent::Failed { at: ea, kind, .. }) => {
                    inner.terminal_at == Some(*ea) && inner.failure_kind == *kind
                }
                (StateKind::Cancelled, TurnEvent::Cancelled { at: ea, .. }) => {
                    inner.terminal_at == Some(*ea)
                }
                _ => false,
            };
            if is_duplicate_settlement {
                if revision > inner.revision {
                    // Idempotent advance must still go through Statig so
                    // evidence/timestamps are not diverged via direct mutation.
                    // Handle the event through the machine to advance revision
                    // without changing terminal evidence.
                    self.machine.handle(&event);
                }
                return Ok(vec![]);
            }
            return Err(TurnError::Sealed {
                state: state_kind,
                event: event_name,
            });
        }

        self.machine.handle(&event);
        Ok(vec![])
    }
}
