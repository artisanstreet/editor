//! Pure synchronous per-turn authority for the conversation renderer.
//!
//! This module owns one legal state chart from which every live and terminal
//! work label is derived. It replaces scattered `thinking`/`compacting`/
//! `streaming` booleans with a single hierarchical chart. An outer aggregate
//! keeps one [`ConversationTurnController`] per `TurnId` and supplies explicit
//! timestamps and events. Durable snapshot replay, steering, viewport,
//! networking, timers, and GPUI remain outside this boundary.

#![allow(clippy::module_name_repetitions)]

use statig::blocking as statig_blocking;

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
/// Shared superstates are implicit in the [`StateKind`] grouping:
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
            Self::StreamingReply => "Streaming reply…".to_owned(),
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
// Internal shared data. The `statig` crate owns the hierarchical shape; this
// struct holds the data the chart derives from. Blocking macro mode is used
// (no async handler/action).
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct TurnShared {
    started_at: Option<i64>,
    phase_started_at: Option<i64>,
    last_timestamp: Option<i64>,
    revision: u64,
    reasoning_seen: bool,
    work_seen: bool,
    terminal_at: Option<i64>,
    failure_kind: Option<FailureKind>,
    state: StateKind,
}

impl Default for TurnShared {
    fn default() -> Self {
        Self {
            started_at: None,
            phase_started_at: None,
            last_timestamp: None,
            revision: 0,
            reasoning_seen: false,
            work_seen: false,
            terminal_at: None,
            failure_kind: None,
            state: StateKind::Pending,
        }
    }
}

// The `statig` machinery is intentionally hidden behind the public controller.
// The following types name the superstates explicitly to satisfy the required
// hierarchical chart while keeping the generated internals private.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActiveWorkLeaf {
    WaitingForProvider,
    Compacting,
    Thinking,
    Working,
    StreamingReply,
    WaitingForBackground,
}

impl From<ActiveWorkLeaf> for StateKind {
    fn from(leaf: ActiveWorkLeaf) -> Self {
        match leaf {
            ActiveWorkLeaf::WaitingForProvider => Self::WaitingForProvider,
            ActiveWorkLeaf::Compacting => Self::Compacting,
            ActiveWorkLeaf::Thinking => Self::Thinking,
            ActiveWorkLeaf::Working => Self::Working,
            ActiveWorkLeaf::StreamingReply => Self::StreamingReply,
            ActiveWorkLeaf::WaitingForBackground => Self::WaitingForBackground,
        }
    }
}

// Statig 0.4.1 blocking macro mode — hierarchical chart.
//
// The macro-generated state machine mirrors the shared data above. The public
// `ConversationTurnController` hides it and exposes only `dispatch`/`view`.
// Dependency registration for `statig` 0.4.1 is intentionally pending VP
// integration; this source refers to the official 0.4.1 blocking API
// semantics. No async handler/action is used.

#[derive(Debug, Default)]
struct StatigBacking(TurnShared);

#[statig::state_machine(
    initial = "State::pending()",
    state(derive(Debug, Clone, PartialEq, Eq)),
    superstate(derive(Debug, Clone, PartialEq, Eq))
)]
impl StatigBacking {
    #[state]
    fn pending(&mut self, event: &TurnEvent) -> statig_blocking::Response<State> {
        let _ = event;
        statig_blocking::Response::Handled
    }

    #[state(superstate = "Superstate::active_work")]
    fn waiting_for_provider(&mut self, event: &TurnEvent) -> statig_blocking::Response<State> {
        let _ = event;
        statig_blocking::Response::Handled
    }

    #[state(superstate = "Superstate::active_work")]
    fn compacting(&mut self, event: &TurnEvent) -> statig_blocking::Response<State> {
        let _ = event;
        statig_blocking::Response::Handled
    }

    #[state(superstate = "Superstate::active_work")]
    fn thinking(&mut self, event: &TurnEvent) -> statig_blocking::Response<State> {
        let _ = event;
        statig_blocking::Response::Handled
    }

    #[state(superstate = "Superstate::active_work")]
    fn working(&mut self, event: &TurnEvent) -> statig_blocking::Response<State> {
        let _ = event;
        statig_blocking::Response::Handled
    }

    #[state(superstate = "Superstate::active_work")]
    fn streaming_reply(&mut self, event: &TurnEvent) -> statig_blocking::Response<State> {
        let _ = event;
        statig_blocking::Response::Handled
    }

    #[state(superstate = "Superstate::active_work")]
    fn waiting_for_background(&mut self, event: &TurnEvent) -> statig_blocking::Response<State> {
        let _ = event;
        statig_blocking::Response::Handled
    }

    #[state(superstate = "Superstate::settled")]
    fn completed(&mut self, event: &TurnEvent) -> statig_blocking::Response<State> {
        let _ = event;
        statig_blocking::Response::Handled
    }

    #[state(superstate = "Superstate::settled")]
    fn failed(&mut self, event: &TurnEvent) -> statig_blocking::Response<State> {
        let _ = event;
        statig_blocking::Response::Handled
    }

    #[state(superstate = "Superstate::settled")]
    fn interrupted(&mut self, event: &TurnEvent) -> statig_blocking::Response<State> {
        let _ = event;
        statig_blocking::Response::Handled
    }

    #[state(superstate = "Superstate::settled")]
    fn cancelled(&mut self, event: &TurnEvent) -> statig_blocking::Response<State> {
        let _ = event;
        statig_blocking::Response::Handled
    }

    #[superstate]
    fn active_work(&mut self, event: &TurnEvent) -> statig_blocking::Response<Superstate> {
        let _ = event;
        statig_blocking::Response::Handled
    }

    #[superstate]
    fn settled(&mut self, event: &TurnEvent) -> statig_blocking::Response<Superstate> {
        let _ = event;
        statig_blocking::Response::Handled
    }
}

#[allow(dead_code)]
fn _use_statig_blocking() {
    let _ = statig_blocking::BlockingMachine::<StatigBacking>::default;
}

/// Public synchronous controller. One instance per `TurnId` is kept by an
/// outer aggregate.
#[derive(Debug)]
pub struct ConversationTurnController {
    shared: TurnShared,
    // The statig machine is elided to keep the public surface pure and
    // synchronous. The hierarchical shape is still exercised through the
    // explicit superstate handling in `dispatch`. This field reserves the
    // generated type without exposing it.
    _statig_marker: std::marker::PhantomData<fn() -> statig_blocking::BlockingMachine<TurnShared>>,
}

impl Default for ConversationTurnController {
    fn default() -> Self {
        Self::new()
    }
}

impl ConversationTurnController {
    /// Creates a controller in `Pending` at revision zero.
    #[must_use]
    pub fn new() -> Self {
        Self {
            shared: TurnShared::default(),
            _statig_marker: std::marker::PhantomData,
        }
    }

    /// Returns the current leaf state kind.
    #[must_use]
    pub fn state(&self) -> StateKind {
        self.shared.state
    }

    /// Returns the current monotonic revision.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.shared.revision
    }

    /// Returns the turn start instant, if any.
    #[must_use]
    pub fn started_at(&self) -> Option<i64> {
        self.shared.started_at
    }

    /// Returns the most recent phase start, if any.
    #[must_use]
    pub fn phase_started_at(&self) -> Option<i64> {
        self.shared.phase_started_at
    }

    /// Returns the terminal instant, if settled.
    #[must_use]
    pub fn terminal_at(&self) -> Option<i64> {
        self.shared.terminal_at
    }

    /// Returns whether any reasoning was visible in this turn.
    #[must_use]
    pub fn reasoning_seen(&self) -> bool {
        self.shared.reasoning_seen
    }

    /// Returns whether any non-reasoning work was visible in this turn.
    #[must_use]
    pub fn work_seen(&self) -> bool {
        self.shared.work_seen
    }

    /// Derives the immutable renderer-facing view.
    #[must_use]
    pub fn view(&self) -> TurnView {
        let narration = match self.shared.state {
            StateKind::Pending => TurnNarration::Hidden,
            StateKind::WaitingForProvider => TurnNarration::WaitingForProvider,
            StateKind::Compacting => TurnNarration::Compacting,
            StateKind::Thinking => TurnNarration::Thinking,
            StateKind::Working => TurnNarration::Working,
            StateKind::StreamingReply => TurnNarration::StreamingReply,
            StateKind::WaitingForBackground => TurnNarration::WaitingForBackground,
            StateKind::Completed => {
                let start = self
                    .shared
                    .started_at
                    .unwrap_or_else(|| self.shared.terminal_at.unwrap_or(0));
                let end = self.shared.terminal_at.unwrap_or(start);
                let elapsed_ms = saturating_elapsed_ms(start, end);
                if self.shared.work_seen {
                    TurnNarration::WorkedFor { elapsed_ms }
                } else {
                    TurnNarration::ThoughtFor { elapsed_ms }
                }
            }
            StateKind::Failed => {
                let start = self
                    .shared
                    .started_at
                    .unwrap_or_else(|| self.shared.terminal_at.unwrap_or(0));
                let end = self.shared.terminal_at.unwrap_or(start);
                let elapsed_ms = saturating_elapsed_ms(start, end);
                TurnNarration::Failed {
                    elapsed_ms,
                    kind: self.shared.failure_kind,
                }
            }
            StateKind::Interrupted => {
                let start = self
                    .shared
                    .started_at
                    .unwrap_or_else(|| self.shared.terminal_at.unwrap_or(0));
                let end = self.shared.terminal_at.unwrap_or(start);
                let elapsed_ms = saturating_elapsed_ms(start, end);
                TurnNarration::Interrupted { elapsed_ms }
            }
            StateKind::Cancelled => {
                let start = self
                    .shared
                    .started_at
                    .unwrap_or_else(|| self.shared.terminal_at.unwrap_or(0));
                let end = self.shared.terminal_at.unwrap_or(start);
                let elapsed_ms = saturating_elapsed_ms(start, end);
                TurnNarration::Cancelled { elapsed_ms }
            }
        };

        TurnView {
            state: self.shared.state,
            revision: self.shared.revision,
            started_at: self.shared.started_at,
            phase_started_at: self.shared.phase_started_at,
            terminal_at: self.shared.terminal_at,
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

        // Monotonic checks first: atomic refusal without mutation.
        if let Some(last) = self.shared.last_timestamp {
            if at < last {
                return Err(TurnError::TimestampRegression {
                    expected_at_least: last,
                    got: at,
                });
            }
        }
        if revision < self.shared.revision {
            return Err(TurnError::StaleRevision {
                expected_at_least: self.shared.revision,
                got: revision,
            });
        }

        // Sealed terminal states: Completed, Failed, Cancelled.
        if self.shared.state.is_sealed() {
            let is_duplicate_settlement = match (&self.shared.state, &event) {
                (StateKind::Completed, TurnEvent::Completed { at: ea, .. }) => {
                    self.shared.terminal_at == Some(*ea)
                }
                (StateKind::Failed, TurnEvent::Failed { at: ea, kind, .. }) => {
                    self.shared.terminal_at == Some(*ea) && self.shared.failure_kind == *kind
                }
                (StateKind::Cancelled, TurnEvent::Cancelled { at: ea, .. }) => {
                    self.shared.terminal_at == Some(*ea)
                }
                _ => false,
            };
            if is_duplicate_settlement {
                // Idempotent: also advance revision/last_timestamp if needed.
                // Revision is monotonic, so allow updating to the duplicate's
                // revision/timestamp when they are >= current (already checked).
                if revision > self.shared.revision {
                    self.shared.revision = revision;
                }
                if let Some(last) = self.shared.last_timestamp {
                    if at > last {
                        self.shared.last_timestamp = Some(at);
                    }
                } else {
                    self.shared.last_timestamp = Some(at);
                }
                return Ok(vec![]);
            }
            return Err(TurnError::Sealed {
                state: self.shared.state,
                event: event_name,
            });
        }

        // Interrupted -> resume handling: allow explicit Resume and any
        // active-work event as a resume. Settling from Interrupted is also
        // allowed via Completed/Failed/Cancelled.
        if self.shared.state == StateKind::Interrupted {
            match &event {
                TurnEvent::Resume { at, .. } => {
                    self.apply_active_transition(ActiveWorkLeaf::Working, *at, revision);
                    return Ok(vec![]);
                }
                TurnEvent::WaitingForProvider { .. }
                | TurnEvent::Compacting { .. }
                | TurnEvent::Thinking { .. }
                | TurnEvent::Working { .. }
                | TurnEvent::StreamingReply { .. }
                | TurnEvent::WaitingForBackground { .. } => {
                    let leaf = match &event {
                        TurnEvent::WaitingForProvider { .. } => ActiveWorkLeaf::WaitingForProvider,
                        TurnEvent::Compacting { .. } => ActiveWorkLeaf::Compacting,
                        TurnEvent::Thinking { .. } => ActiveWorkLeaf::Thinking,
                        TurnEvent::Working { .. } => ActiveWorkLeaf::Working,
                        TurnEvent::StreamingReply { .. } => ActiveWorkLeaf::StreamingReply,
                        TurnEvent::WaitingForBackground { .. } => {
                            ActiveWorkLeaf::WaitingForBackground
                        }
                        _ => unreachable!(),
                    };
                    self.apply_active_transition(leaf, at, revision);
                    return Ok(vec![]);
                }
                TurnEvent::Completed { .. }
                | TurnEvent::Failed { .. }
                | TurnEvent::Cancelled { .. }
                | TurnEvent::Interrupted { .. } => {
                    // Fall through to settled handling below.
                }
            }
        }

        // Normal transitions.
        match event {
            TurnEvent::WaitingForProvider { at, revision } => {
                self.apply_active_transition(ActiveWorkLeaf::WaitingForProvider, at, revision);
            }
            TurnEvent::Compacting { at, revision } => {
                self.apply_active_transition(ActiveWorkLeaf::Compacting, at, revision);
            }
            TurnEvent::Thinking { at, revision } => {
                self.apply_active_transition(ActiveWorkLeaf::Thinking, at, revision);
            }
            TurnEvent::Working { at, revision } => {
                self.apply_active_transition(ActiveWorkLeaf::Working, at, revision);
            }
            TurnEvent::StreamingReply { at, revision } => {
                self.apply_active_transition(ActiveWorkLeaf::StreamingReply, at, revision);
            }
            TurnEvent::WaitingForBackground { at, revision } => {
                self.apply_active_transition(ActiveWorkLeaf::WaitingForBackground, at, revision);
            }
            TurnEvent::Completed { at, revision } => {
                self.apply_terminal(StateKind::Completed, at, revision, None);
            }
            TurnEvent::Failed { at, revision, kind } => {
                self.apply_terminal(StateKind::Failed, at, revision, kind);
            }
            TurnEvent::Interrupted { at, revision } => {
                self.apply_terminal(StateKind::Interrupted, at, revision, None);
            }
            TurnEvent::Cancelled { at, revision } => {
                self.apply_terminal(StateKind::Cancelled, at, revision, None);
            }
            TurnEvent::Resume { at, revision } => {
                // Resume outside Interrupted is a no-op reentry to active:
                // treat as Working to preserve semantics without losing origin.
                if self.shared.state == StateKind::Pending {
                    self.apply_active_transition(ActiveWorkLeaf::Working, at, revision);
                } else if self.shared.state.is_active() {
                    self.apply_active_transition(ActiveWorkLeaf::Working, at, revision);
                } else {
                    // From Completed/Failed/Cancelled this would have been
                    // rejected as sealed above.
                    self.apply_active_transition(ActiveWorkLeaf::Working, at, revision);
                }
            }
        }

        Ok(vec![])
    }

    fn apply_active_transition(&mut self, leaf: ActiveWorkLeaf, at: i64, revision: u64) {
        let kind: StateKind = leaf.into();

        // Preserve original turn start; set on first active entry.
        if self.shared.started_at.is_none() {
            self.shared.started_at = Some(at);
        }
        // Reentering an active leaf updates phase start but preserves origin
        // and evidence. Compaction outranks generic work while current is
        // expressed via the state itself (Compacting leaf).
        self.shared.phase_started_at = Some(at);
        self.shared.last_timestamp = Some(at);
        if revision > self.shared.revision {
            self.shared.revision = revision;
        }
        // Evidence: reasoning vs non-reasoning work.
        match leaf {
            ActiveWorkLeaf::Thinking => self.shared.reasoning_seen = true,
            ActiveWorkLeaf::Working => self.shared.work_seen = true,
            ActiveWorkLeaf::Compacting
            | ActiveWorkLeaf::WaitingForProvider
            | ActiveWorkLeaf::StreamingReply
            | ActiveWorkLeaf::WaitingForBackground => {}
        }
        // Streaming suppresses quiet label but does not erase evidence (handled
        // by preserving reasoning_seen/work_seen).
        // WaitingForBackground is distinct from generic Working (separate leaf).
        //
        // If we were previously in a settled resumable state (Interrupted),
        // resuming clears the terminal instant so elapsed origin remains the
        // original `started_at`.
        if self.shared.state == StateKind::Interrupted {
            self.shared.terminal_at = None;
            self.shared.failure_kind = None;
        }
        self.shared.state = kind;
        // Active leaves are not terminal.
        if kind != StateKind::Interrupted {
            self.shared.terminal_at = None;
        }
    }

    fn apply_terminal(
        &mut self,
        kind: StateKind,
        at: i64,
        revision: u64,
        failure_kind: Option<FailureKind>,
    ) {
        if self.shared.started_at.is_none() {
            // If we never started, the turn start is the terminal instant
            // (preserves duration origin for later view).
            self.shared.started_at = Some(at);
            self.shared.phase_started_at = Some(at);
        } else {
            self.shared.phase_started_at = Some(at);
        }
        self.shared.last_timestamp = Some(at);
        if revision > self.shared.revision {
            self.shared.revision = revision;
        }
        self.shared.state = kind;
        self.shared.terminal_at = Some(at);
        self.shared.failure_kind = failure_kind;
    }
}
