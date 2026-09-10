//! Frontend presentation state paired from engine subscription events.
//!
//! [`EngineObservationState`] is the GPUI-side boundary between an
//! already-decoded [`EngineObservationEvent`](artisan_domain::EngineObservationEvent)
//! stream and the renderer-facing rows it owns. It performs no I/O,
//! scheduling, retry, clock, logging, serialization, or payload decoding: the
//! values it pairs are the validated, sanitized S1a domain vocabulary, so no
//! provider payload beyond that boundary can reach a renderer through this
//! module.
//!
//! Pairing rules:
//!
//! - Every event names its durable [`ObservationSequence`](artisan_domain::ObservationSequence)
//!   and arrives with the one-based [`EventCursor`](artisan_protocol::EventCursor)
//!   minted by the delivery connection. Application in cursor order with
//!   duplicate suppression makes reconnect replay idempotent.
//! - Approval and question rows are keyed by their provider `approval_id` and
//!   `question_id`. A resolution settles its requested row in place and never
//!   duplicates it, so a later answer packet can attach by request id.
//! - Message and reasoning deltas accumulate per item; a completion settles
//!   the accumulated text authoritatively.
//! - Usage reports fold into [`UsageTotals`] honoring their
//!   [`UsageBasis`](artisan_domain::UsageBasis): delta counts add,
//!   cumulative counts replace, and the context-window gauge always replaces.
//! - Unknown future arms degrade to a diagnostic timeline row through
//!   [`EngineObservationState::record_unknown`] and never panic. The protocol
//!   codec already rejects unknown wire discriminants with typed errors, so
//!   this seam covers future [`Observation`](artisan_domain::Observation)
//!   variants mapped by later packets.
//!
//! This module issues no commands and performs no RPCs. Answering an approval
//! or question (the A-approve packet) attaches to the request ids retained
//! here; no answer dispatch exists on this path.

#![allow(clippy::module_name_repetitions)]

use std::collections::{HashMap, HashSet};

use artisan_domain::{
    ApprovalKind as DomainApprovalKind, ApprovalObservation, ApprovalState as DomainApprovalState,
    EngineObservationAttribution, EngineObservationEvent, MessagePhase, Observation,
    QuestionObservation, QuestionState as DomainQuestionState, RunId, RunState, RunTerminalState,
    TerminalActivityObservation, TerminalActivityState, ThreadId, ToolAction, ToolObservation,
    TurnId, TurnState, UnixMillis, UsageBasis, UsageObservation,
};

use crate::approval_presentation::{
    ApprovalItem, ApprovalKind as PresentationKind, ApprovalPresentation,
    ApprovalRequest as PresentationRequest, ApprovalState as PresentationState,
    get_approval_presentation,
};

/// Outcome of applying one subscription event to presentation state.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ApplyOutcome {
    /// The event extended or settled presentation state.
    Applied {
        /// Stable observation tag that was paired.
        tag: &'static str,
        /// Whether a previously requested row settled in place.
        settled_in_place: bool,
    },
    /// The event was already applied or precedes the applied cursor.
    Duplicate,
    /// The event names a thread other than the owned one.
    StaleThread,
}

/// Counts from applying one reconnect replay batch in cursor order.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct ReplaySummary {
    /// Events that extended or settled presentation state.
    pub applied: usize,
    /// Events skipped as already applied or preceding the applied cursor.
    pub duplicates: usize,
    /// Events skipped for naming another thread.
    pub stale: usize,
}

/// Scopes one provider row key by its Forge run.
///
/// Attributed rows key as `run_id#provider_id` so two runs sharing provider
/// item names cannot coalesce. Legacy rows without attribution keep the bare
/// provider id so existing pairing behavior is preserved.
#[must_use]
pub fn scoped_row_key(
    attribution: Option<&EngineObservationAttribution>,
    provider_id: &str,
) -> String {
    match attribution {
        Some(attr) => format!("{}#{provider_id}", attr.run_id.as_str()),
        None => provider_id.to_owned(),
    }
}

/// Orders one replay entry: attributed lanes by durable delivery sequence,
/// legacy lanes by wire cursor.
///
/// Attributed entries sort before legacy entries; within each lane the
/// durable order applies. Cross-lane order is irrelevant to pairing
/// correctness because each lane carries its own dedup.
fn replay_order_key(event: &EngineObservationEvent, cursor: u64) -> (u8, u64, u64) {
    match &event.attribution {
        Some(attr) => (0, attr.delivery_sequence, cursor),
        None => (1, cursor, cursor),
    }
}

/// Orders one explicit-attribution replay entry, falling back to the event's
/// own attribution when the override is [`None`].
fn attributed_replay_order_key(
    event: &EngineObservationEvent,
    cursor: u64,
    attribution: Option<&EngineObservationAttribution>,
) -> (u8, u64, u64) {
    match attribution.or(event.attribution.as_ref()) {
        Some(attr) => (0, attr.delivery_sequence, cursor),
        None => (1, cursor, cursor),
    }
}

/// One accumulating agent message keyed by its native item id.
#[derive(Clone, Debug, PartialEq)]
pub struct MessageRow {
    item_id: String,
    phase: MessagePhase,
    text: String,
    completed: bool,
    turn_id: String,
    cursor: u64,
    sequence: u64,
    attribution: Option<EngineObservationAttribution>,
}

impl MessageRow {
    /// Returns the native assistant message item this row extends.
    #[must_use]
    pub fn item_id(&self) -> &str {
        &self.item_id
    }

    /// Returns the provider-disclosed display phase of the latest event.
    #[must_use]
    pub const fn phase(&self) -> MessagePhase {
        self.phase
    }

    /// Returns the accumulated or settled message text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Returns whether a completion has settled this row.
    #[must_use]
    pub const fn completed(&self) -> bool {
        self.completed
    }

    /// Returns the provider turn identity.
    #[must_use]
    pub fn turn_id(&self) -> &str {
        &self.turn_id
    }

    /// Returns the full delivery attribution, when the delivery carried one.
    ///
    /// Attribution is all-or-none: a real DB delivery carries the exact
    /// Forge run/turn/time/sequence, while legacy transport carries none.
    #[must_use]
    pub const fn attribution(&self) -> Option<&EngineObservationAttribution> {
        self.attribution.as_ref()
    }

    /// Returns the attributed Forge run, when the delivery carried one.
    #[must_use]
    pub fn attributed_run(&self) -> Option<&RunId> {
        self.attribution.as_ref().map(|attr| &attr.run_id)
    }

    /// Returns the attributed canonical Forge turn, when delivered.
    #[must_use]
    pub fn attributed_turn(&self) -> Option<&TurnId> {
        self.attribution.as_ref().map(|attr| &attr.turn_id)
    }

    /// Returns the durable commit time, when delivered.
    #[must_use]
    pub fn committed_at(&self) -> Option<UnixMillis> {
        self.attribution.as_ref().map(|attr| attr.committed_at)
    }

    /// Returns the thread-scoped delivery sequence, when delivered.
    #[must_use]
    pub fn delivery_sequence(&self) -> Option<u64> {
        self.attribution.as_ref().map(|attr| attr.delivery_sequence)
    }
}

/// One accumulating reasoning summary keyed by its reasoning item id.
#[derive(Clone, Debug, PartialEq)]
pub struct ReasoningRow {
    item_id: String,
    text: String,
    settled: bool,
    turn_id: String,
    cursor: u64,
    sequence: u64,
    attribution: Option<EngineObservationAttribution>,
}

impl ReasoningRow {
    /// Returns the reasoning item this row extends.
    #[must_use]
    pub fn item_id(&self) -> &str {
        &self.item_id
    }

    /// Returns the accumulated or authoritative public summary text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Returns whether a completion has settled this row.
    #[must_use]
    pub const fn settled(&self) -> bool {
        self.settled
    }

    /// Returns the provider turn identity.
    #[must_use]
    pub fn turn_id(&self) -> &str {
        &self.turn_id
    }

    /// Returns the full delivery attribution, when the delivery carried one.
    ///
    /// Attribution is all-or-none: a real DB delivery carries the exact
    /// Forge run/turn/time/sequence, while legacy transport carries none.
    #[must_use]
    pub const fn attribution(&self) -> Option<&EngineObservationAttribution> {
        self.attribution.as_ref()
    }

    /// Returns the attributed Forge run, when the delivery carried one.
    #[must_use]
    pub fn attributed_run(&self) -> Option<&RunId> {
        self.attribution.as_ref().map(|attr| &attr.run_id)
    }

    /// Returns the attributed canonical Forge turn, when delivered.
    #[must_use]
    pub fn attributed_turn(&self) -> Option<&TurnId> {
        self.attribution.as_ref().map(|attr| &attr.turn_id)
    }

    /// Returns the durable commit time, when delivered.
    #[must_use]
    pub fn committed_at(&self) -> Option<UnixMillis> {
        self.attribution.as_ref().map(|attr| attr.committed_at)
    }

    /// Returns the thread-scoped delivery sequence, when delivered.
    #[must_use]
    pub fn delivery_sequence(&self) -> Option<u64> {
        self.attribution.as_ref().map(|attr| attr.delivery_sequence)
    }
}

/// Latest lifecycle report for one tool invocation, keyed by tool id.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolRow {
    tool_id: String,
    tool_name: String,
    action: ToolAction,
    detail: Option<String>,
    cursor: u64,
    sequence: u64,
    attribution: Option<EngineObservationAttribution>,
}

impl ToolRow {
    /// Returns the provider tool invocation identity.
    #[must_use]
    pub fn tool_id(&self) -> &str {
        &self.tool_id
    }

    /// Returns the provider tool name.
    #[must_use]
    pub fn tool_name(&self) -> &str {
        &self.tool_name
    }

    /// Returns the latest lifecycle action.
    #[must_use]
    pub const fn action(&self) -> ToolAction {
        self.action
    }

    /// Returns the latest provider detail, when disclosed.
    #[must_use]
    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }

    /// Returns the full delivery attribution, when the delivery carried one.
    ///
    /// Attribution is all-or-none: a real DB delivery carries the exact
    /// Forge run/turn/time/sequence, while legacy transport carries none.
    #[must_use]
    pub const fn attribution(&self) -> Option<&EngineObservationAttribution> {
        self.attribution.as_ref()
    }

    /// Returns the attributed Forge run, when the delivery carried one.
    #[must_use]
    pub fn attributed_run(&self) -> Option<&RunId> {
        self.attribution.as_ref().map(|attr| &attr.run_id)
    }

    /// Returns the attributed canonical Forge turn, when delivered.
    #[must_use]
    pub fn attributed_turn(&self) -> Option<&TurnId> {
        self.attribution.as_ref().map(|attr| &attr.turn_id)
    }

    /// Returns the durable commit time, when delivered.
    #[must_use]
    pub fn committed_at(&self) -> Option<UnixMillis> {
        self.attribution.as_ref().map(|attr| attr.committed_at)
    }

    /// Returns the thread-scoped delivery sequence, when delivered.
    #[must_use]
    pub fn delivery_sequence(&self) -> Option<u64> {
        self.attribution.as_ref().map(|attr| attr.delivery_sequence)
    }
}

/// Latest activity for one terminal session, keyed by activity id.
#[derive(Clone, Debug, PartialEq)]
pub struct TerminalRow {
    activity_id: String,
    command: Option<String>,
    shell: Option<String>,
    output: String,
    exit_code: Option<i32>,
    state: TerminalActivityState,
    cursor: u64,
    sequence: u64,
    attribution: Option<EngineObservationAttribution>,
}

impl TerminalRow {
    /// Returns the provider activity identity.
    #[must_use]
    pub fn activity_id(&self) -> &str {
        &self.activity_id
    }

    /// Returns the latest disclosed command text.
    #[must_use]
    pub fn command(&self) -> Option<&str> {
        self.command.as_deref()
    }

    /// Returns the latest disclosed interpreter name.
    #[must_use]
    pub fn shell(&self) -> Option<&str> {
        self.shell.as_deref()
    }

    /// Returns the accumulated output chunks in arrival order.
    #[must_use]
    pub fn output(&self) -> &str {
        &self.output
    }

    /// Returns the latest reported process exit code.
    #[must_use]
    pub const fn exit_code(&self) -> Option<i32> {
        self.exit_code
    }

    /// Returns the latest activity lifecycle state.
    #[must_use]
    pub const fn state(&self) -> TerminalActivityState {
        self.state
    }

    /// Returns the full delivery attribution, when the delivery carried one.
    ///
    /// Attribution is all-or-none: a real DB delivery carries the exact
    /// Forge run/turn/time/sequence, while legacy transport carries none.
    #[must_use]
    pub const fn attribution(&self) -> Option<&EngineObservationAttribution> {
        self.attribution.as_ref()
    }

    /// Returns the attributed Forge run, when the delivery carried one.
    #[must_use]
    pub fn attributed_run(&self) -> Option<&RunId> {
        self.attribution.as_ref().map(|attr| &attr.run_id)
    }

    /// Returns the attributed canonical Forge turn, when delivered.
    #[must_use]
    pub fn attributed_turn(&self) -> Option<&TurnId> {
        self.attribution.as_ref().map(|attr| &attr.turn_id)
    }

    /// Returns the durable commit time, when delivered.
    #[must_use]
    pub fn committed_at(&self) -> Option<UnixMillis> {
        self.attribution.as_ref().map(|attr| attr.committed_at)
    }

    /// Returns the thread-scoped delivery sequence, when delivered.
    #[must_use]
    pub fn delivery_sequence(&self) -> Option<u64> {
        self.attribution.as_ref().map(|attr| attr.delivery_sequence)
    }
}

/// One approval request with its eventual decision, keyed by approval id.
///
/// The row renders with its provider `approval_id` so the later answer packet
/// can attach a decision to it. No answer dispatch exists here.
#[derive(Clone, Debug, PartialEq)]
pub struct ApprovalRow {
    approval_id: String,
    description: String,
    request: artisan_domain::ApprovalRequest,
    approved: Option<bool>,
    cursor: u64,
    sequence: u64,
    attribution: Option<EngineObservationAttribution>,
}

impl ApprovalRow {
    /// Returns the provider approval identity answers attach to.
    #[must_use]
    pub fn approval_id(&self) -> &str {
        &self.approval_id
    }

    /// Returns the human-readable approval description.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Returns the provider-neutral action under review.
    #[must_use]
    pub const fn request(&self) -> &artisan_domain::ApprovalRequest {
        &self.request
    }

    /// Returns the decision for resolved approvals, [`None`] while requested.
    #[must_use]
    pub const fn approved(&self) -> Option<bool> {
        self.approved
    }

    /// Returns whether the row still awaits a decision.
    #[must_use]
    pub const fn is_requested(&self) -> bool {
        self.approved.is_none()
    }

    /// Returns the full delivery attribution, when the delivery carried one.
    ///
    /// Attribution is all-or-none: a real DB delivery carries the exact
    /// Forge run/turn/time/sequence, while legacy transport carries none.
    #[must_use]
    pub const fn attribution(&self) -> Option<&EngineObservationAttribution> {
        self.attribution.as_ref()
    }

    /// Returns the attributed Forge run, when the delivery carried one.
    #[must_use]
    pub fn attributed_run(&self) -> Option<&RunId> {
        self.attribution.as_ref().map(|attr| &attr.run_id)
    }

    /// Returns the attributed canonical Forge turn, when delivered.
    #[must_use]
    pub fn attributed_turn(&self) -> Option<&TurnId> {
        self.attribution.as_ref().map(|attr| &attr.turn_id)
    }

    /// Returns the durable commit time, when delivered.
    #[must_use]
    pub fn committed_at(&self) -> Option<UnixMillis> {
        self.attribution.as_ref().map(|attr| attr.committed_at)
    }

    /// Returns the thread-scoped delivery sequence, when delivered.
    #[must_use]
    pub fn delivery_sequence(&self) -> Option<u64> {
        self.attribution.as_ref().map(|attr| attr.delivery_sequence)
    }

    /// Projects this row into the pure approval presentation policy.
    ///
    /// The mapping pairs with
    /// [`approval_presentation`](crate::approval_presentation) instead of
    /// forking it: domain kinds map one for one, the domain description
    /// becomes the legacy prompt, and an undecided row is requested while a
    /// decided row is approved or rejected.
    #[must_use]
    pub fn presentation_item(&self) -> ApprovalItem {
        let kind = match self.request.kind() {
            DomainApprovalKind::Command => PresentationKind::Command,
            DomainApprovalKind::FileChange => PresentationKind::FileChange,
            DomainApprovalKind::Action => PresentationKind::Action,
        };
        let state = match self.approved {
            None => PresentationState::Requested,
            Some(true) => PresentationState::Approved,
            Some(false) => PresentationState::Rejected,
        };
        ApprovalItem::new(
            self.description.clone(),
            state,
            Some(PresentationRequest::new(
                kind,
                self.request.command_text().map(str::to_owned),
                self.request.cwd().map(str::to_owned),
                self.request.reason().map(str::to_owned),
            )),
        )
    }

    /// Computes the exact renderer-facing presentation for this row.
    #[must_use]
    pub fn presentation(&self) -> ApprovalPresentation {
        get_approval_presentation(&self.presentation_item())
    }
}

/// One renderer-safe question option.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct QuestionOptionView {
    label: String,
    description: Option<String>,
}

impl QuestionOptionView {
    /// Returns the option label.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Returns what choosing this option means, when explained.
    #[must_use]
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }
}

/// One question request with its eventual answers, keyed by question id.
///
/// Like approvals, the row renders with its provider `question_id` so the
/// later answer packet can attach to it. No answer dispatch exists here.
#[derive(Clone, Debug, PartialEq)]
pub struct QuestionRow {
    question_id: String,
    text: String,
    header: Option<String>,
    multi_select: bool,
    options: Option<Vec<QuestionOptionView>>,
    answers: Option<Vec<String>>,
    cursor: u64,
    sequence: u64,
    attribution: Option<EngineObservationAttribution>,
}

impl QuestionRow {
    /// Returns the provider question identity answers attach to.
    #[must_use]
    pub fn question_id(&self) -> &str {
        &self.question_id
    }

    /// Returns the question itself.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Returns the short category label, when disclosed.
    #[must_use]
    pub fn header(&self) -> Option<&str> {
        self.header.as_deref()
    }

    /// Returns whether more than one option may be chosen at once.
    #[must_use]
    pub const fn multi_select(&self) -> bool {
        self.multi_select
    }

    /// Returns the offered answers, or [`None`] for a free-form question.
    #[must_use]
    pub fn options(&self) -> Option<&[QuestionOptionView]> {
        self.options.as_deref()
    }

    /// Returns the answers for resolved questions, [`None`] while requested.
    #[must_use]
    pub fn answers(&self) -> Option<&[String]> {
        self.answers.as_deref()
    }

    /// Returns whether the row still awaits answers.
    #[must_use]
    pub const fn is_requested(&self) -> bool {
        self.answers.is_none()
    }

    /// Returns the full delivery attribution, when the delivery carried one.
    ///
    /// Attribution is all-or-none: a real DB delivery carries the exact
    /// Forge run/turn/time/sequence, while legacy transport carries none.
    #[must_use]
    pub const fn attribution(&self) -> Option<&EngineObservationAttribution> {
        self.attribution.as_ref()
    }

    /// Returns the attributed Forge run, when the delivery carried one.
    #[must_use]
    pub fn attributed_run(&self) -> Option<&RunId> {
        self.attribution.as_ref().map(|attr| &attr.run_id)
    }

    /// Returns the attributed canonical Forge turn, when delivered.
    #[must_use]
    pub fn attributed_turn(&self) -> Option<&TurnId> {
        self.attribution.as_ref().map(|attr| &attr.turn_id)
    }

    /// Returns the durable commit time, when delivered.
    #[must_use]
    pub fn committed_at(&self) -> Option<UnixMillis> {
        self.attribution.as_ref().map(|attr| attr.committed_at)
    }

    /// Returns the thread-scoped delivery sequence, when delivered.
    #[must_use]
    pub fn delivery_sequence(&self) -> Option<u64> {
        self.attribution.as_ref().map(|attr| attr.delivery_sequence)
    }
}

/// Folded provider usage for the run.
///
/// Delta counts add to the folded totals while cumulative counts replace
/// them. The context-window gauge always replaces, regardless of basis, so a
/// newer report never sums with an older one.
#[derive(Clone, Debug, PartialEq)]
pub struct UsageTotals {
    reports: usize,
    basis: UsageBasis,
    input_tokens: u64,
    cached_input_tokens: u64,
    output_tokens: u64,
    context_tokens: Option<u64>,
    context_window_tokens: Option<u64>,
    cost_usd: f64,
}

impl UsageTotals {
    /// Returns how many usage reports have folded into the totals.
    #[must_use]
    pub const fn reports(&self) -> usize {
        self.reports
    }

    /// Returns the accounting basis of the latest report.
    #[must_use]
    pub const fn basis(&self) -> UsageBasis {
        self.basis
    }

    /// Returns the folded input token count.
    #[must_use]
    pub const fn input_tokens(&self) -> u64 {
        self.input_tokens
    }

    /// Returns the folded cached input token count.
    #[must_use]
    pub const fn cached_input_tokens(&self) -> u64 {
        self.cached_input_tokens
    }

    /// Returns the folded output token count.
    #[must_use]
    pub const fn output_tokens(&self) -> u64 {
        self.output_tokens
    }

    /// Returns the latest context gauge, never a sum across reports.
    #[must_use]
    pub const fn context_tokens(&self) -> Option<u64> {
        self.context_tokens
    }

    /// Returns the latest disclosed usable context window.
    #[must_use]
    pub const fn context_window_tokens(&self) -> Option<u64> {
        self.context_window_tokens
    }

    /// Returns the folded cost in US dollars.
    #[must_use]
    pub const fn cost_usd(&self) -> f64 {
        self.cost_usd
    }
}

/// One discrete timeline row for observations without settle semantics.
///
/// File, search, plan, compaction, retry, run and turn states, subagent
/// activity, native actions, diagnostics, terminal outcomes, and degraded
/// unknown arms all render as ordered rows. Every summary is projected from
/// the sanitized S1a vocabulary; no other provider payload is retained.
#[derive(Clone, Debug, PartialEq)]
pub struct TimelineRow {
    cursor: u64,
    sequence: Option<u64>,
    tag: &'static str,
    summary: String,
    attribution: Option<EngineObservationAttribution>,
}

impl TimelineRow {
    /// Returns the delivery cursor that published this row.
    #[must_use]
    pub const fn cursor(&self) -> u64 {
        self.cursor
    }

    /// Returns the durable observation sequence, when the row pairs one.
    #[must_use]
    pub const fn sequence(&self) -> Option<u64> {
        self.sequence
    }

    /// Returns the stable observation tag, or `"unknown"` for degraded arms.
    #[must_use]
    pub const fn tag(&self) -> &'static str {
        self.tag
    }

    /// Returns the renderer-safe summary.
    #[must_use]
    pub fn summary(&self) -> &str {
        &self.summary
    }

    /// Returns the full delivery attribution, when the delivery carried one.
    ///
    /// Attribution is all-or-none: a real DB delivery carries the exact
    /// Forge run/turn/time/sequence, while legacy transport carries none.
    #[must_use]
    pub const fn attribution(&self) -> Option<&EngineObservationAttribution> {
        self.attribution.as_ref()
    }

    /// Returns the attributed Forge run, when the delivery carried one.
    #[must_use]
    pub fn attributed_run(&self) -> Option<&RunId> {
        self.attribution.as_ref().map(|attr| &attr.run_id)
    }

    /// Returns the attributed canonical Forge turn, when delivered.
    #[must_use]
    pub fn attributed_turn(&self) -> Option<&TurnId> {
        self.attribution.as_ref().map(|attr| &attr.turn_id)
    }

    /// Returns the durable commit time, when delivered.
    #[must_use]
    pub fn committed_at(&self) -> Option<UnixMillis> {
        self.attribution.as_ref().map(|attr| attr.committed_at)
    }

    /// Returns the thread-scoped delivery sequence, when delivered.
    #[must_use]
    pub fn delivery_sequence(&self) -> Option<u64> {
        self.attribution.as_ref().map(|attr| attr.delivery_sequence)
    }
}

/// Latest terminal outcome for the run, when one has arrived.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RunTerminalView {
    /// The terminal outcome.
    pub state: RunTerminalState,
    /// Whether the engine produced a session title by settle time.
    pub has_summary_title: bool,
}

/// Presentation state paired from one thread's engine subscription events.
///
/// The state is renderer-owned and thread-scoped: events naming another
/// thread report [`ApplyOutcome::StaleThread`] and change nothing.
pub struct EngineObservationState {
    thread_id: ThreadId,
    last_cursor: u64,
    seen_ids: HashSet<String>,
    /// Durable delivery sequences already paired for attributed rows.
    ///
    /// The backend writer resets the wire cursor to 1 on every new
    /// connection, so attributed reconnect dedup keys on this thread-scoped
    /// strictly increasing sequence plus the stable observation identity,
    /// never on the wire cursor.
    seen_delivery_sequences: HashSet<u64>,
    messages: HashMap<String, MessageRow>,
    message_order: Vec<String>,
    reasoning: HashMap<String, ReasoningRow>,
    reasoning_order: Vec<String>,
    tools: HashMap<String, ToolRow>,
    tool_order: Vec<String>,
    terminals: HashMap<String, TerminalRow>,
    terminal_order: Vec<String>,
    approvals: HashMap<String, ApprovalRow>,
    approval_order: Vec<String>,
    questions: HashMap<String, QuestionRow>,
    question_order: Vec<String>,
    turn_states: HashMap<String, TurnState>,
    run_state: Option<RunState>,
    run_terminal: Option<RunTerminalView>,
    usage: UsageTotals,
    timeline: Vec<TimelineRow>,
}

impl EngineObservationState {
    /// Creates empty presentation state for `thread_id` with no cursor.
    #[must_use]
    pub fn new(thread_id: ThreadId) -> Self {
        Self {
            thread_id,
            last_cursor: 0,
            seen_ids: HashSet::new(),
            seen_delivery_sequences: HashSet::new(),
            messages: HashMap::new(),
            message_order: Vec::new(),
            reasoning: HashMap::new(),
            reasoning_order: Vec::new(),
            tools: HashMap::new(),
            tool_order: Vec::new(),
            terminals: HashMap::new(),
            terminal_order: Vec::new(),
            approvals: HashMap::new(),
            approval_order: Vec::new(),
            questions: HashMap::new(),
            question_order: Vec::new(),
            turn_states: HashMap::new(),
            run_state: None,
            run_terminal: None,
            usage: UsageTotals {
                reports: 0,
                basis: UsageBasis::Unknown,
                input_tokens: 0,
                cached_input_tokens: 0,
                output_tokens: 0,
                context_tokens: None,
                context_window_tokens: None,
                cost_usd: 0.0,
            },
            timeline: Vec::new(),
        }
    }

    /// Returns the thread whose subscription events this state pairs.
    #[must_use]
    pub const fn thread_id(&self) -> &ThreadId {
        &self.thread_id
    }

    /// Returns the highest delivery cursor applied so far, if any.
    #[must_use]
    pub fn last_cursor(&self) -> Option<u64> {
        (self.last_cursor > 0).then_some(self.last_cursor)
    }

    /// Returns the total number of presentation rows retained.
    #[must_use]
    pub fn row_count(&self) -> usize {
        self.message_order.len()
            + self.reasoning_order.len()
            + self.tool_order.len()
            + self.terminal_order.len()
            + self.approval_order.len()
            + self.question_order.len()
            + self.timeline.len()
    }

    /// Returns whether any presentation row has been paired.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.row_count() == 0
    }

    /// Returns the message row for `item_id`, if one has arrived.
    #[must_use]
    pub fn message(&self, item_id: &str) -> Option<&MessageRow> {
        self.messages.get(item_id)
    }

    /// Returns message rows in first-seen order.
    #[must_use]
    pub fn messages_in_order(&self) -> Vec<&MessageRow> {
        self.ordered(&self.message_order, &self.messages)
    }

    /// Returns the reasoning row for `item_id`, if one has arrived.
    ///
    /// Legacy lookups use the bare provider id. Attributed rows are scoped by
    /// Forge run; use [`Self::reasoning_scoped`] for an exact run-scoped read.
    #[must_use]
    pub fn reasoning(&self, item_id: &str) -> Option<&ReasoningRow> {
        self.reasoning.get(item_id)
    }

    /// Returns the run-scoped reasoning row for one Forge run.
    #[must_use]
    pub fn reasoning_scoped(&self, run_id: &RunId, item_id: &str) -> Option<&ReasoningRow> {
        self.reasoning
            .get(&format!("{}#{item_id}", run_id.as_str()))
    }

    /// Returns reasoning rows in first-seen order.
    #[must_use]
    pub fn reasoning_in_order(&self) -> Vec<&ReasoningRow> {
        self.ordered(&self.reasoning_order, &self.reasoning)
    }

    /// Returns the tool row for `tool_id`, if one has arrived.
    ///
    /// Legacy lookups use the bare provider id. Attributed rows are scoped by
    /// Forge run; use [`Self::tool_scoped`] for an exact run-scoped read.
    #[must_use]
    pub fn tool(&self, tool_id: &str) -> Option<&ToolRow> {
        self.tools.get(tool_id)
    }

    /// Returns the run-scoped tool row for one Forge run.
    #[must_use]
    pub fn tool_scoped(&self, run_id: &RunId, tool_id: &str) -> Option<&ToolRow> {
        self.tools.get(&format!("{}#{tool_id}", run_id.as_str()))
    }

    /// Returns tool rows in first-seen order.
    #[must_use]
    pub fn tools_in_order(&self) -> Vec<&ToolRow> {
        self.ordered(&self.tool_order, &self.tools)
    }

    /// Returns the terminal row for `activity_id`, if one has arrived.
    ///
    /// Legacy lookups use the bare provider id. Attributed rows are scoped by
    /// Forge run; use [`Self::terminal_scoped`] for an exact run-scoped read.
    #[must_use]
    pub fn terminal(&self, activity_id: &str) -> Option<&TerminalRow> {
        self.terminals.get(activity_id)
    }

    /// Returns the run-scoped terminal row for one Forge run.
    #[must_use]
    pub fn terminal_scoped(&self, run_id: &RunId, activity_id: &str) -> Option<&TerminalRow> {
        self.terminals
            .get(&format!("{}#{activity_id}", run_id.as_str()))
    }

    /// Returns terminal rows in first-seen order.
    #[must_use]
    pub fn terminals_in_order(&self) -> Vec<&TerminalRow> {
        self.ordered(&self.terminal_order, &self.terminals)
    }

    /// Returns the approval row for `approval_id`, if one has arrived.
    #[must_use]
    pub fn approval(&self, approval_id: &str) -> Option<&ApprovalRow> {
        self.approvals.get(approval_id)
    }

    /// Returns approval rows in first-requested order.
    #[must_use]
    pub fn approvals_in_order(&self) -> Vec<&ApprovalRow> {
        self.ordered(&self.approval_order, &self.approvals)
    }

    /// Returns the renderer-facing presentation for `approval_id`, if known.
    #[must_use]
    pub fn approval_presentation(&self, approval_id: &str) -> Option<ApprovalPresentation> {
        self.approval(approval_id).map(ApprovalRow::presentation)
    }

    /// Returns the question row for `question_id`, if one has arrived.
    #[must_use]
    pub fn question(&self, question_id: &str) -> Option<&QuestionRow> {
        self.questions.get(question_id)
    }

    /// Returns question rows in first-requested order.
    #[must_use]
    pub fn questions_in_order(&self) -> Vec<&QuestionRow> {
        self.ordered(&self.question_order, &self.questions)
    }

    /// Returns the latest lifecycle state for `turn_id`, if one has arrived.
    #[must_use]
    pub fn turn_state(&self, turn_id: &str) -> Option<TurnState> {
        self.turn_states.get(turn_id).copied()
    }

    /// Returns the latest non-terminal run state, if one has arrived.
    #[must_use]
    pub const fn run_state(&self) -> Option<RunState> {
        self.run_state
    }

    /// Returns the terminal run outcome, if one has arrived.
    #[must_use]
    pub const fn run_terminal(&self) -> Option<RunTerminalView> {
        self.run_terminal
    }

    /// Returns the folded provider usage.
    #[must_use]
    pub const fn usage(&self) -> &UsageTotals {
        &self.usage
    }

    /// Returns discrete timeline rows in cursor application order.
    #[must_use]
    pub fn timeline(&self) -> &[TimelineRow] {
        &self.timeline
    }

    fn ordered<'state, Row>(
        &'state self,
        order: &[String],
        rows: &'state HashMap<String, Row>,
    ) -> Vec<&'state Row> {
        order
            .iter()
            .filter_map(|key| rows.get(key))
            .collect::<Vec<&'state Row>>()
    }

    /// Applies one subscription event at its delivery cursor.
    ///
    /// The attribution is read from the event itself: real DB deliveries
    /// carry `Some` with the exact Forge run/turn/time/sequence, while legacy
    /// transport carries `None` and pairs typed rows without activity
    /// identity (never fabricating activity facts). Dedup follows the event
    /// kind: attributed rows use the durable thread-scoped delivery sequence
    /// plus stable observation identity, because the backend writer resets
    /// the wire cursor to 1 on every new connection; legacy rows use wire
    /// cursor order only.
    #[must_use]
    pub fn apply(&mut self, cursor: u64, event: &EngineObservationEvent) -> ApplyOutcome {
        self.apply_attributed(cursor, event, event.attribution.as_ref())
    }

    /// Applies one subscription event with explicit Forge attribution.
    ///
    /// An explicit `Some` attribution overrides the event's own attribution;
    /// [`None`] falls back to the event's attribution, so legacy events pair
    /// without activity identity. Attributed rows scope by Forge run, so two
    /// runs sharing provider item names cannot coalesce.
    #[must_use]
    pub fn apply_attributed(
        &mut self,
        cursor: u64,
        event: &EngineObservationEvent,
        attribution: Option<&EngineObservationAttribution>,
    ) -> ApplyOutcome {
        if event.thread_id != self.thread_id {
            return ApplyOutcome::StaleThread;
        }
        let effective = attribution.or(event.attribution.as_ref());
        if let Some(attr) = effective {
            let observation_id = event.observation.observation_id().as_str().to_owned();
            if self
                .seen_delivery_sequences
                .contains(&attr.delivery_sequence)
                || !self.seen_ids.insert(observation_id)
            {
                return ApplyOutcome::Duplicate;
            }
            self.seen_delivery_sequences.insert(attr.delivery_sequence);
            let sequence = event.observation.sequence().get();
            let (tag, settled_in_place) =
                self.pair(cursor, sequence, &event.observation, Some(attr));
            return ApplyOutcome::Applied {
                tag,
                settled_in_place,
            };
        }
        if cursor == 0 || cursor <= self.last_cursor {
            return ApplyOutcome::Duplicate;
        }
        if !self
            .seen_ids
            .insert(event.observation.observation_id().as_str().to_owned())
        {
            return ApplyOutcome::Duplicate;
        }
        self.last_cursor = cursor;
        let sequence = event.observation.sequence().get();
        let (tag, settled_in_place) = self.pair(cursor, sequence, &event.observation, None);
        ApplyOutcome::Applied {
            tag,
            settled_in_place,
        }
    }

    /// Applies one reconnect replay batch in durable order with dedup.
    ///
    /// Attributed entries apply first in delivery-sequence order, then legacy
    /// entries in wire-cursor order; each lane keeps its own internal order
    /// and dedup, so a replay that arrives out of order still settles rows in
    /// durable order. Duplicate and stale entries are counted, never applied
    /// twice.
    #[must_use]
    pub fn apply_replay(&mut self, mut batch: Vec<(u64, EngineObservationEvent)>) -> ReplaySummary {
        batch.sort_by(|left, right| {
            replay_order_key(&left.1, left.0).cmp(&replay_order_key(&right.1, right.0))
        });
        let mut summary = ReplaySummary::default();
        for (cursor, event) in &batch {
            match self.apply(*cursor, event) {
                ApplyOutcome::Applied { .. } => {
                    summary.applied = summary.applied.saturating_add(1);
                }
                ApplyOutcome::Duplicate => {
                    summary.duplicates = summary.duplicates.saturating_add(1);
                }
                ApplyOutcome::StaleThread => {
                    summary.stale = summary.stale.saturating_add(1);
                }
            }
        }
        summary
    }

    /// Applies one attributed reconnect replay batch in durable order.
    ///
    /// Each entry carries an explicit Forge attribution override; [`None`]
    /// entries fall back to the event's own attribution. Ordering and
    /// counting match [`Self::apply_replay`].
    #[must_use]
    pub fn apply_attributed_replay(
        &mut self,
        mut batch: Vec<(
            u64,
            EngineObservationEvent,
            Option<EngineObservationAttribution>,
        )>,
    ) -> ReplaySummary {
        batch.sort_by(|left, right| {
            let left_key = attributed_replay_order_key(&left.1, left.0, left.2.as_ref());
            let right_key = attributed_replay_order_key(&right.1, right.0, right.2.as_ref());
            left_key.cmp(&right_key)
        });
        let mut summary = ReplaySummary::default();
        for (cursor, event, attribution) in &batch {
            match self.apply_attributed(*cursor, event, attribution.as_ref()) {
                ApplyOutcome::Applied { .. } => {
                    summary.applied = summary.applied.saturating_add(1);
                }
                ApplyOutcome::Duplicate => {
                    summary.duplicates = summary.duplicates.saturating_add(1);
                }
                ApplyOutcome::StaleThread => {
                    summary.stale = summary.stale.saturating_add(1);
                }
            }
        }
        summary
    }

    /// Degrades one unknown future event arm to a diagnostic timeline row.
    ///
    /// The row retains only the stable tag and a renderer-safe detail; it
    /// never panics and never retains a provider payload. Cursor and thread
    /// guards match [`Self::apply`].
    #[must_use]
    pub fn record_unknown(
        &mut self,
        thread_id: &ThreadId,
        cursor: u64,
        tag: &'static str,
        detail: String,
    ) -> ApplyOutcome {
        if thread_id != &self.thread_id {
            return ApplyOutcome::StaleThread;
        }
        if cursor == 0 || cursor <= self.last_cursor {
            return ApplyOutcome::Duplicate;
        }
        self.last_cursor = cursor;
        self.timeline.push(TimelineRow {
            cursor,
            sequence: None,
            tag,
            summary: detail,
            attribution: None,
        });
        ApplyOutcome::Applied {
            tag: "unknown",
            settled_in_place: false,
        }
    }

    fn pair(
        &mut self,
        cursor: u64,
        sequence: u64,
        observation: &Observation,
        attribution: Option<&EngineObservationAttribution>,
    ) -> (&'static str, bool) {
        match observation {
            Observation::AgentMessageDelta(value) => {
                self.pair_message_delta(cursor, sequence, value, attribution);
                (observation.tag(), false)
            }
            Observation::AgentMessageCompleted(value) => {
                self.pair_message_completed(cursor, sequence, value, attribution);
                (observation.tag(), true)
            }
            Observation::Approval(value) => {
                let settled = self.pair_approval(cursor, sequence, value, attribution);
                (observation.tag(), settled)
            }
            Observation::Compaction(value) => {
                self.push_compaction(cursor, sequence, observation.tag(), value, attribution)
            }
            Observation::File(value) => {
                self.pair_file(cursor, sequence, value, attribution);
                (observation.tag(), false)
            }
            Observation::NativeAction(value) => {
                self.push_native_action(cursor, sequence, observation.tag(), value, attribution)
            }
            Observation::Plan(value) => {
                self.push_plan(cursor, sequence, observation.tag(), value, attribution)
            }
            Observation::ProcessDiagnostic(value) => self.push_process_diagnostic(
                cursor,
                sequence,
                observation.tag(),
                value,
                attribution,
            ),
            Observation::ProtocolDiagnostic(value) => self.push_protocol_diagnostic(
                cursor,
                sequence,
                observation.tag(),
                value,
                attribution,
            ),
            Observation::Question(value) => {
                let settled = self.pair_question(cursor, sequence, value, attribution);
                (observation.tag(), settled)
            }
            Observation::ReasoningSummaryCompleted(value) => {
                self.pair_reasoning_completed(cursor, sequence, value, attribution);
                (observation.tag(), true)
            }
            Observation::ReasoningSummaryDelta(value) => {
                self.pair_reasoning_delta(cursor, sequence, value, attribution);
                (observation.tag(), false)
            }
            Observation::Retry(value) => {
                self.push_retry(cursor, sequence, observation.tag(), value, attribution)
            }
            Observation::RunState(value) => {
                self.push_run_state(cursor, sequence, observation.tag(), value, attribution)
            }
            Observation::RunTerminal(value) => {
                self.push_run_terminal(cursor, sequence, observation.tag(), value, attribution)
            }
            Observation::Search(value) => {
                self.pair_search(cursor, sequence, value, attribution);
                (observation.tag(), false)
            }
            Observation::Subagent(value) => {
                self.push_subagent(cursor, sequence, observation.tag(), value, attribution)
            }
            Observation::SubagentTranscript(value) => self.push_subagent_transcript(
                cursor,
                sequence,
                observation.tag(),
                value,
                attribution,
            ),
            Observation::TerminalActivity(value) => {
                self.pair_terminal(cursor, sequence, value, attribution);
                (observation.tag(), false)
            }
            Observation::Tool(value) => {
                self.pair_tool(cursor, sequence, value, attribution);
                (observation.tag(), false)
            }
            Observation::TurnState(value) => {
                self.push_turn_state(cursor, sequence, observation.tag(), value, attribution)
            }
            Observation::Usage(value) => {
                self.pair_usage(value);
                (observation.tag(), false)
            }
        }
    }

    fn push_compaction(
        &mut self,
        cursor: u64,
        sequence: u64,
        tag: &'static str,
        value: &artisan_domain::CompactionObservation,
        attribution: Option<&EngineObservationAttribution>,
    ) -> (&'static str, bool) {
        let summary = format!("compaction {}", value.state().as_str());
        self.push_timeline(cursor, sequence, tag, summary, attribution);
        (tag, false)
    }

    fn push_native_action(
        &mut self,
        cursor: u64,
        sequence: u64,
        tag: &'static str,
        value: &artisan_domain::NativeActionObservation,
        attribution: Option<&EngineObservationAttribution>,
    ) -> (&'static str, bool) {
        let mut summary = format!("native action {}", value.action());
        if let Some(detail) = value.detail() {
            summary.push_str(": ");
            summary.push_str(detail);
        }
        self.push_timeline(cursor, sequence, tag, summary, attribution);
        (tag, false)
    }

    fn push_plan(
        &mut self,
        cursor: u64,
        sequence: u64,
        tag: &'static str,
        value: &artisan_domain::PlanObservation,
        attribution: Option<&EngineObservationAttribution>,
    ) -> (&'static str, bool) {
        let summary = format!("plan with {} entries", value.entries().len());
        self.push_timeline(cursor, sequence, tag, summary, attribution);
        (tag, false)
    }

    fn push_process_diagnostic(
        &mut self,
        cursor: u64,
        sequence: u64,
        tag: &'static str,
        value: &artisan_domain::ProcessDiagnosticObservation,
        attribution: Option<&EngineObservationAttribution>,
    ) -> (&'static str, bool) {
        let summary = format!("[{}] {}", value.level().as_str(), value.message());
        self.push_timeline(cursor, sequence, tag, summary, attribution);
        (tag, false)
    }

    fn push_protocol_diagnostic(
        &mut self,
        cursor: u64,
        sequence: u64,
        tag: &'static str,
        value: &artisan_domain::ProtocolDiagnosticObservation,
        attribution: Option<&EngineObservationAttribution>,
    ) -> (&'static str, bool) {
        let summary = format!("[{}] {}", value.level().as_str(), value.message());
        self.push_timeline(cursor, sequence, tag, summary, attribution);
        (tag, false)
    }

    fn push_retry(
        &mut self,
        cursor: u64,
        sequence: u64,
        tag: &'static str,
        value: &artisan_domain::RetryObservation,
        attribution: Option<&EngineObservationAttribution>,
    ) -> (&'static str, bool) {
        let summary = format!("{}: {}", value.attempt_state().as_str(), value.message());
        self.push_timeline(cursor, sequence, tag, summary, attribution);
        (tag, false)
    }

    fn push_run_state(
        &mut self,
        cursor: u64,
        sequence: u64,
        tag: &'static str,
        value: &artisan_domain::RunStateObservation,
        attribution: Option<&EngineObservationAttribution>,
    ) -> (&'static str, bool) {
        self.run_state = Some(value.state());
        let summary = format!("run {}", value.state().as_str());
        self.push_timeline(cursor, sequence, tag, summary, attribution);
        (tag, false)
    }

    fn push_run_terminal(
        &mut self,
        cursor: u64,
        sequence: u64,
        tag: &'static str,
        value: &artisan_domain::RunTerminalObservation,
        attribution: Option<&EngineObservationAttribution>,
    ) -> (&'static str, bool) {
        self.run_terminal = Some(RunTerminalView {
            state: value.state(),
            has_summary_title: value.summary_title().is_some(),
        });
        let mut summary = format!("run {}", value.state().as_str());
        if let Some(title) = value.summary_title() {
            summary.push_str(": ");
            summary.push_str(title);
        }
        self.push_timeline(cursor, sequence, tag, summary, attribution);
        (tag, false)
    }

    fn push_subagent(
        &mut self,
        cursor: u64,
        sequence: u64,
        tag: &'static str,
        value: &artisan_domain::SubagentObservation,
        attribution: Option<&EngineObservationAttribution>,
    ) -> (&'static str, bool) {
        let mut summary = format!(
            "subagent {} {}",
            value.agent_native_thread_id().as_str(),
            value.state().as_str()
        );
        if let Some(activity) = value.activity() {
            summary.push_str(": ");
            summary.push_str(activity);
        }
        self.push_timeline(cursor, sequence, tag, summary, attribution);
        (tag, false)
    }

    fn push_subagent_transcript(
        &mut self,
        cursor: u64,
        sequence: u64,
        tag: &'static str,
        value: &artisan_domain::SubagentTranscriptObservation,
        attribution: Option<&EngineObservationAttribution>,
    ) -> (&'static str, bool) {
        let summary = format!(
            "subagent {} transcript",
            value.agent_native_thread_id().as_str()
        );
        self.push_timeline(cursor, sequence, tag, summary, attribution);
        (tag, false)
    }

    fn push_turn_state(
        &mut self,
        cursor: u64,
        sequence: u64,
        tag: &'static str,
        value: &artisan_domain::TurnStateObservation,
        attribution: Option<&EngineObservationAttribution>,
    ) -> (&'static str, bool) {
        self.turn_states
            .insert(value.turn_id().as_str().to_owned(), value.state());
        let summary = format!(
            "turn {} {}",
            value.turn_id().as_str(),
            value.state().as_str()
        );
        self.push_timeline(cursor, sequence, tag, summary, attribution);
        (tag, false)
    }

    fn pair_message_delta(
        &mut self,
        cursor: u64,
        sequence: u64,
        value: &artisan_domain::AgentMessageDeltaObservation,
        attribution: Option<&EngineObservationAttribution>,
    ) {
        let key = scoped_row_key(attribution, value.item_id().as_str());
        if !self.message_order.iter().any(|known| known == &key) {
            self.message_order.push(key.clone());
        }
        self.messages
            .entry(key)
            .and_modify(|row| {
                row.phase = value.phase();
                row.text.push_str(value.delta());
                row.cursor = cursor;
                row.sequence = sequence;
                row.attribution = attribution.cloned();
            })
            .or_insert_with(|| MessageRow {
                item_id: value.item_id().as_str().to_owned(),
                phase: value.phase(),
                text: value.delta().to_owned(),
                completed: false,
                turn_id: value.turn_id().as_str().to_owned(),
                cursor,
                sequence,
                attribution: attribution.cloned(),
            });
    }

    fn pair_message_completed(
        &mut self,
        cursor: u64,
        sequence: u64,
        value: &artisan_domain::AgentMessageCompletedObservation,
        attribution: Option<&EngineObservationAttribution>,
    ) {
        let key = scoped_row_key(attribution, value.item_id().as_str());
        if !self.message_order.iter().any(|known| known == &key) {
            self.message_order.push(key.clone());
        }
        self.messages
            .entry(key)
            .and_modify(|row| {
                row.phase = value.phase();
                row.text = value.message().to_owned();
                row.completed = true;
                row.cursor = cursor;
                row.sequence = sequence;
                row.attribution = attribution.cloned();
            })
            .or_insert_with(|| MessageRow {
                item_id: value.item_id().as_str().to_owned(),
                phase: value.phase(),
                text: value.message().to_owned(),
                completed: true,
                turn_id: value.turn_id().as_str().to_owned(),
                cursor,
                sequence,
                attribution: attribution.cloned(),
            });
    }

    fn pair_reasoning_delta(
        &mut self,
        cursor: u64,
        sequence: u64,
        value: &artisan_domain::ReasoningSummaryDeltaObservation,
        attribution: Option<&EngineObservationAttribution>,
    ) {
        let key = scoped_row_key(attribution, value.item_id().as_str());
        if !self.reasoning_order.iter().any(|known| known == &key) {
            self.reasoning_order.push(key.clone());
        }
        self.reasoning
            .entry(key)
            .and_modify(|row| {
                row.text.push_str(value.delta());
                row.cursor = cursor;
                row.sequence = sequence;
                row.attribution = attribution.cloned();
            })
            .or_insert_with(|| ReasoningRow {
                item_id: value.item_id().as_str().to_owned(),
                text: value.delta().to_owned(),
                settled: false,
                turn_id: value.turn_id().as_str().to_owned(),
                cursor,
                sequence,
                attribution: attribution.cloned(),
            });
    }

    fn pair_reasoning_completed(
        &mut self,
        cursor: u64,
        sequence: u64,
        value: &artisan_domain::ReasoningSummaryCompletedObservation,
        attribution: Option<&EngineObservationAttribution>,
    ) {
        let key = scoped_row_key(attribution, value.item_id().as_str());
        if !self.reasoning_order.iter().any(|known| known == &key) {
            self.reasoning_order.push(key.clone());
        }
        self.reasoning
            .entry(key)
            .and_modify(|row| {
                if let Some(text) = value.text() {
                    row.text = text.to_owned();
                }
                row.settled = true;
                row.cursor = cursor;
                row.sequence = sequence;
                row.attribution = attribution.cloned();
            })
            .or_insert_with(|| ReasoningRow {
                item_id: value.item_id().as_str().to_owned(),
                text: value.text().unwrap_or_default().to_owned(),
                settled: true,
                turn_id: value.turn_id().as_str().to_owned(),
                cursor,
                sequence,
                attribution: attribution.cloned(),
            });
    }

    fn pair_tool(
        &mut self,
        cursor: u64,
        sequence: u64,
        value: &ToolObservation,
        attribution: Option<&EngineObservationAttribution>,
    ) {
        let key = scoped_row_key(attribution, value.tool_id().as_str());
        if !self.tool_order.iter().any(|known| known == &key) {
            self.tool_order.push(key.clone());
        }
        self.tools
            .entry(key)
            .and_modify(|row| {
                row.action = value.action();
                row.detail = value.detail().map(str::to_owned);
                row.cursor = cursor;
                row.sequence = sequence;
                row.attribution = attribution.cloned();
            })
            .or_insert_with(|| ToolRow {
                tool_id: value.tool_id().as_str().to_owned(),
                tool_name: value.tool_name().to_owned(),
                action: value.action(),
                detail: value.detail().map(str::to_owned),
                cursor,
                sequence,
                attribution: attribution.cloned(),
            });
    }

    fn pair_terminal(
        &mut self,
        cursor: u64,
        sequence: u64,
        value: &TerminalActivityObservation,
        attribution: Option<&EngineObservationAttribution>,
    ) {
        let key = scoped_row_key(attribution, value.activity_id().as_str());
        if !self.terminal_order.iter().any(|known| known == &key) {
            self.terminal_order.push(key.clone());
        }
        self.terminals
            .entry(key)
            .and_modify(|row| {
                if value.command().is_some() {
                    row.command = value.command().map(str::to_owned);
                }
                if value.shell().is_some() {
                    row.shell = value.shell().map(str::to_owned);
                }
                if let Some(chunk) = value.output() {
                    row.output.push_str(chunk);
                }
                if value.exit_code().is_some() {
                    row.exit_code = value.exit_code();
                }
                row.state = value.state();
                row.cursor = cursor;
                row.sequence = sequence;
                row.attribution = attribution.cloned();
            })
            .or_insert_with(|| TerminalRow {
                activity_id: value.activity_id().as_str().to_owned(),
                command: value.command().map(str::to_owned),
                shell: value.shell().map(str::to_owned),
                output: value.output().unwrap_or_default().to_owned(),
                exit_code: value.exit_code(),
                state: value.state(),
                cursor,
                sequence,
                attribution: attribution.cloned(),
            });
    }

    fn pair_approval(
        &mut self,
        cursor: u64,
        sequence: u64,
        value: &ApprovalObservation,
        attribution: Option<&EngineObservationAttribution>,
    ) -> bool {
        let key = scoped_row_key(attribution, value.approval_id().as_str());
        let first_seen = !self.approval_order.iter().any(|known| known == &key);
        if first_seen {
            self.approval_order.push(key.clone());
        }
        let approved = match value.state() {
            DomainApprovalState::Requested => None,
            DomainApprovalState::Resolved => value.approved(),
        };
        let settled_in_place = !first_seen && approved.is_some();
        self.approvals
            .entry(key)
            .and_modify(|row| {
                row.description = value.description().to_owned();
                row.request = value.request().clone();
                row.approved = approved;
                row.cursor = cursor;
                row.sequence = sequence;
                row.attribution = attribution.cloned();
            })
            .or_insert_with(|| ApprovalRow {
                approval_id: value.approval_id().as_str().to_owned(),
                description: value.description().to_owned(),
                request: value.request().clone(),
                approved,
                cursor,
                sequence,
                attribution: attribution.cloned(),
            });
        settled_in_place
    }

    fn pair_question(
        &mut self,
        cursor: u64,
        sequence: u64,
        value: &QuestionObservation,
        attribution: Option<&EngineObservationAttribution>,
    ) -> bool {
        let key = scoped_row_key(attribution, value.question_id().as_str());
        let first_seen = !self.question_order.iter().any(|known| known == &key);
        if first_seen {
            self.question_order.push(key.clone());
        }
        let answers = match value.state() {
            DomainQuestionState::Requested => None,
            DomainQuestionState::Resolved => value.answers().cloned(),
        };
        let settled_in_place = !first_seen && answers.is_some();
        self.questions
            .entry(key)
            .and_modify(|row| {
                row.text = value.text().to_owned();
                row.header = value.header().map(str::to_owned);
                row.multi_select = value.multi_select();
                row.options = value.options().map(|options| {
                    options
                        .iter()
                        .map(|option| QuestionOptionView {
                            label: option.label().to_owned(),
                            description: option.description().map(str::to_owned),
                        })
                        .collect::<Vec<QuestionOptionView>>()
                });
                row.answers = answers.clone();
                row.cursor = cursor;
                row.sequence = sequence;
                row.attribution = attribution.cloned();
            })
            .or_insert_with(|| QuestionRow {
                question_id: value.question_id().as_str().to_owned(),
                text: value.text().to_owned(),
                header: value.header().map(str::to_owned),
                multi_select: value.multi_select(),
                options: value.options().map(|options| {
                    options
                        .iter()
                        .map(|option| QuestionOptionView {
                            label: option.label().to_owned(),
                            description: option.description().map(str::to_owned),
                        })
                        .collect::<Vec<QuestionOptionView>>()
                }),
                answers,
                cursor,
                sequence,
                attribution: attribution.cloned(),
            });
        settled_in_place
    }

    fn pair_usage(&mut self, value: &UsageObservation) {
        let totals = &mut self.usage;
        totals.reports = totals.reports.saturating_add(1);
        totals.basis = value.basis();
        match value.basis() {
            UsageBasis::Delta => {
                totals.input_tokens = totals
                    .input_tokens
                    .saturating_add(value.input_tokens().unwrap_or(0));
                totals.cached_input_tokens = totals
                    .cached_input_tokens
                    .saturating_add(value.cached_input_tokens().unwrap_or(0));
                totals.output_tokens = totals
                    .output_tokens
                    .saturating_add(value.output_tokens().unwrap_or(0));
                totals.cost_usd += value.cost_usd().unwrap_or(0.0);
            }
            UsageBasis::Cumulative | UsageBasis::Unknown => {
                if let Some(input) = value.input_tokens() {
                    totals.input_tokens = input;
                }
                if let Some(cached) = value.cached_input_tokens() {
                    totals.cached_input_tokens = cached;
                }
                if let Some(output) = value.output_tokens() {
                    totals.output_tokens = output;
                }
                if let Some(cost) = value.cost_usd() {
                    totals.cost_usd = cost;
                }
            }
        }
        if value.context_tokens().is_some() {
            totals.context_tokens = value.context_tokens();
        }
        if value.context_window_tokens().is_some() {
            totals.context_window_tokens = value.context_window_tokens();
        }
    }

    fn pair_file(
        &mut self,
        cursor: u64,
        sequence: u64,
        value: &artisan_domain::FileObservation,
        attribution: Option<&EngineObservationAttribution>,
    ) {
        let mut summary = format!("{} {}", value.action().as_str(), value.path());
        match (value.lines_added(), value.lines_deleted()) {
            (Some(added), Some(deleted)) => {
                summary.push_str(&format!(" (+{added}/-{deleted})"));
            }
            (Some(added), None) => {
                summary.push_str(&format!(" (+{added})"));
            }
            (None, Some(deleted)) => {
                summary.push_str(&format!(" (-{deleted})"));
            }
            (None, None) => {}
        }
        self.push_timeline(cursor, sequence, "file", summary, attribution);
    }

    fn pair_search(
        &mut self,
        cursor: u64,
        sequence: u64,
        value: &artisan_domain::SearchObservation,
        attribution: Option<&EngineObservationAttribution>,
    ) {
        let mut summary = format!("search {} \"{}\"", value.state().as_str(), value.query());
        if let Some(scope) = value.scope() {
            summary.push_str(&format!(" [{}]", scope.as_str()));
        }
        if let Some(count) = value.result_count() {
            summary.push_str(&format!(" ({count} results)"));
        }
        self.push_timeline(cursor, sequence, "search", summary, attribution);
    }

    fn push_timeline(
        &mut self,
        cursor: u64,
        sequence: u64,
        tag: &'static str,
        summary: String,
        attribution: Option<&EngineObservationAttribution>,
    ) {
        self.timeline.push(TimelineRow {
            cursor,
            sequence: Some(sequence),
            tag,
            summary,
            attribution: attribution.cloned(),
        });
    }
}
