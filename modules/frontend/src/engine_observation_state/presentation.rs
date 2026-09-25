//! Renderer-facing observation row vocabulary.
//!
//! Extracted verbatim from `engine_observation_state.rs` during the module
//! split.

#[allow(clippy::wildcard_imports)]
use super::*;

/// One accumulating reasoning summary keyed by its reasoning item id.
#[derive(Clone, Debug, PartialEq)]
pub struct ReasoningRow {
    pub(super) item_id: String,
    pub(super) text: String,
    pub(super) settled: bool,
    pub(super) turn_id: String,
    pub(super) cursor: u64,
    pub(super) sequence: u64,
    pub(super) attribution: Option<EngineObservationAttribution>,
    pub(super) first_committed_at: Option<UnixMillis>,
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

    /// Persisted time of the first event for this item; updates keep its position.
    #[must_use]
    pub fn first_committed_at(&self) -> Option<UnixMillis> {
        self.first_committed_at
    }

    /// Returns the latest durable commit time, when delivered.
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
    pub(super) tool_id: String,
    pub(super) tool_name: String,
    pub(super) action: ToolAction,
    pub(super) detail: Option<String>,
    pub(super) cursor: u64,
    pub(super) sequence: u64,
    pub(super) attribution: Option<EngineObservationAttribution>,
    pub(super) first_committed_at: Option<UnixMillis>,
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

    /// Persisted time of the first event for this item; updates keep its position.
    #[must_use]
    pub fn first_committed_at(&self) -> Option<UnixMillis> {
        self.first_committed_at
    }

    /// Returns the latest durable commit time, when delivered.
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
    pub(super) activity_id: String,
    pub(super) command: Option<String>,
    pub(super) shell: Option<String>,
    pub(super) output: String,
    pub(super) exit_code: Option<i32>,
    pub(super) state: TerminalActivityState,
    pub(super) cursor: u64,
    pub(super) sequence: u64,
    pub(super) attribution: Option<EngineObservationAttribution>,
    pub(super) first_committed_at: Option<UnixMillis>,
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

    /// Persisted time of the first event for this item; updates keep its position.
    #[must_use]
    pub fn first_committed_at(&self) -> Option<UnixMillis> {
        self.first_committed_at
    }

    /// Returns the latest durable commit time, when delivered.
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
    pub(super) approval_id: String,
    pub(super) description: String,
    pub(super) request: artisan_domain::ApprovalRequest,
    pub(super) approved: Option<bool>,
    pub(super) cursor: u64,
    pub(super) sequence: u64,
    pub(super) attribution: Option<EngineObservationAttribution>,
    pub(super) first_committed_at: Option<UnixMillis>,
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

    /// Persisted time of the first event for this item; updates keep its position.
    #[must_use]
    pub fn first_committed_at(&self) -> Option<UnixMillis> {
        self.first_committed_at
    }

    /// Returns the latest durable commit time, when delivered.
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
    pub(super) label: String,
    pub(super) description: Option<String>,
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
    pub(super) question_id: String,
    pub(super) text: String,
    pub(super) header: Option<String>,
    pub(super) multi_select: bool,
    pub(super) options: Option<Vec<QuestionOptionView>>,
    pub(super) answers: Option<Vec<String>>,
    pub(super) cursor: u64,
    pub(super) sequence: u64,
    pub(super) attribution: Option<EngineObservationAttribution>,
    pub(super) first_committed_at: Option<UnixMillis>,
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

    /// Persisted time of the first event for this item; updates keep its position.
    #[must_use]
    pub fn first_committed_at(&self) -> Option<UnixMillis> {
        self.first_committed_at
    }

    /// Returns the latest durable commit time, when delivered.
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

/// One discrete timeline row for observations without settle semantics.
///
/// File, search, plan, compaction, retry, run and turn states, subagent
/// activity, native actions, diagnostics, terminal outcomes, and degraded
/// unknown arms all render as ordered rows. Every summary is projected from
/// the sanitized S1a vocabulary; no other provider payload is retained.
#[derive(Clone, Debug, PartialEq)]
pub struct TimelineRow {
    pub(super) cursor: u64,
    pub(super) sequence: Option<u64>,
    pub(super) tag: &'static str,
    pub(super) summary: String,
    pub(super) attribution: Option<EngineObservationAttribution>,
    pub(super) first_committed_at: Option<UnixMillis>,
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

    /// Persisted time of the first event for this item; updates keep its position.
    #[must_use]
    pub fn first_committed_at(&self) -> Option<UnixMillis> {
        self.first_committed_at
    }

    /// Returns the latest durable commit time, when delivered.
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
