//! Finite A-approve UI: GPUI answer actions plus requested-to-resolved pairing.
//!
//! This module owns the native approve-answer surface and nothing else. Two
//! GPUI actions ([`RespondApprovalAction`] and [`RespondQuestionAction`])
//! carry one explicit authenticated user gesture each into the existing
//! domain commands ([`Command::RespondApproval`] and
//! [`Command::RespondQuestion`]) through the existing transport/request path:
//! every answer attempt mints a fresh client [`RequestId`] so retries
//! correlate to a receipt instead of a second effect. There is deliberately
//! no default decision, no auto-answer, and no ambient thread/run binding:
//! the owning thread is supplied explicitly at dispatch, and the decision or
//! answers travel only inside the action value.
//!
//! Receipt pairing interprets one correlated [`RespondApprovalReceipt`] or
//! [`RespondQuestionReceipt`] (outcome plus disposition) against the command
//! that was dispatched, and reports an [`AnswerSettlement`] alongside the
//! S1c requested row it pairs onto
//! ([`EngineObservationState`](crate::engine_observation_state::EngineObservationState)).
//! Pairing never mutates subscription state itself: durable resolutions
//! settle rows in place through the subscription observations, while this
//! module reports whether the answer settled, conflicts, must wait for a
//! live run, may retry, or degrades to a diagnostic. Unknown future receipt
//! arms degrade to [`AnswerSettlement::Diagnostic`], never panic.
//!
//! Rendered states project pending [`ApprovalRow`](crate::engine_observation_state::ApprovalRow)
//! and [`QuestionRow`](crate::engine_observation_state::QuestionRow) rows
//! through the shared [`ApprovalPresentation`] policy instead of forking it:
//! pending approvals expose approve/deny affordances, pending questions
//! expose option choices (honoring multi-select) or free-form entry where no
//! options were offered. Only the sanitized S1c row vocabulary reaches these
//! views; no other provider payload is retained.

use artisan_domain::{
    Command, ObservationId, ReceiptDisposition, RequestId, RespondApproval, RespondQuestion, RunId,
    RunInteractionError, ThreadId,
};
use artisan_protocol::{
    ErrorCode, ProtocolFailure, RespondApprovalReceipt, RespondQuestionReceipt,
    RunInteractionOutcome,
};
use thiserror::Error;

use crate::approval_presentation::{ApprovalKind, ApprovalPresentation};
use crate::engine_observation_state::{
    ApprovalRow, EngineObservationState, QuestionOptionView, QuestionRow,
};

/// GPUI action answering one pending approval with an explicit decision.
///
/// This is one authenticated user gesture: `approved` is always stated, never
/// defaulted. The owning thread and the client-minted request identity join
/// at dispatch through [`approval_command`].
#[derive(Clone, Debug, Eq, PartialEq, gpui::Action)]
#[action(no_json, name = "respond_approval")]
pub struct RespondApprovalAction {
    /// Exact native run identity owning the pending approval.
    pub run_id: RunId,
    /// Provider approval identity under review.
    pub approval_id: ObservationId,
    /// The explicit decision: true allows, false denies.
    pub approved: bool,
}

/// GPUI action answering one pending question with explicit answers.
///
/// An empty `answers` list records an explicitly skipped question, mirroring
/// [`RespondQuestion`](artisan_domain::RespondQuestion). Choice and free-form
/// surfaces must only dispatch non-empty lists for ordinary answers (as in
/// `conversation-prompt.svelte`) and reserve the empty list for an explicit
/// skip gesture, so an answer is never synthesized.
#[derive(Clone, Debug, Eq, PartialEq, gpui::Action)]
#[action(no_json, name = "respond_question")]
pub struct RespondQuestionAction {
    /// Exact native run identity owning the pending question.
    pub run_id: RunId,
    /// Provider question identity under review.
    pub question_id: ObservationId,
    /// The explicit answers, possibly empty for a skipped question.
    pub answers: Vec<String>,
}

/// Which pending provider request an answer gesture settles.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AnswerKind {
    /// A pending approval request.
    Approval,
    /// A pending question.
    Question,
}

impl AnswerKind {
    /// Returns the stable target spelling shared with the domain vocabulary.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Approval => "approval",
            Self::Question => "question",
        }
    }

    /// Returns the failure prefix used when a response cannot be recorded.
    #[must_use]
    pub const fn failure_prefix(self) -> &'static str {
        match self {
            Self::Approval => "Could not respond to approval: ",
            Self::Question => "Could not respond to question: ",
        }
    }
}

/// Failure minting one client request identity for an answer attempt.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum AnswerIntentError {
    /// No answer request identity could be minted.
    #[error("no further answer request identity can be minted")]
    IdentityExhausted,
}

/// Visible label of the denial affordance.
///
/// The shared [`ApprovalPresentation`] carries only the affirmative label
/// (`Run command`, `Apply changes`, `Approve`); the complementary explicit
/// gesture is this constant, matching `conversation-approval.svelte`.
pub const APPROVAL_DENY_LABEL: &str = "Deny";

/// In-flight label while a denial is settling, matching the legacy surface.
pub const APPROVAL_DENYING_LABEL: &str = "Denying…";

/// Label for the multi-select and free-form confirmation control.
pub const QUESTION_ANSWER_LABEL: &str = "Answer";

/// Accessible label of the free-form question input.
pub const QUESTION_INPUT_LABEL: &str = "Answer question";

/// Placeholder of the free-form question input.
pub const QUESTION_INPUT_PLACEHOLDER: &str = "Type your answer";

/// Mints one fresh client request identity for an answer attempt.
///
/// Every answer attempt mints exactly one identity; an explicit retry of that
/// attempt reuses the minted identity so the retry correlates to a receipt
/// instead of a second effect.
///
/// # Errors
///
/// Returns [`AnswerIntentError::IdentityExhausted`] if the identity cannot be
/// formed (never for this fixed label).
pub fn mint_answer_request_id() -> Result<RequestId, AnswerIntentError> {
    RequestId::mint("native-answer").map_err(|_| AnswerIntentError::IdentityExhausted)
}

/// Builds the domain approval command for one explicit answer gesture.
///
/// The owning thread is bound explicitly here; nothing ambient is read. The
/// decision travels only from `action`, so silence can never approve or deny.
#[must_use]
pub fn approval_command(
    thread_id: ThreadId,
    action: &RespondApprovalAction,
    request_id: RequestId,
) -> Command {
    Command::RespondApproval(RespondApproval::new(
        request_id,
        thread_id,
        action.run_id.clone(),
        action.approval_id.clone(),
        action.approved,
    ))
}

/// Builds the domain question command for one explicit answer gesture.
///
/// An empty answer list is accepted as an explicitly skipped question; answer
/// bounds are validated exactly like the domain command.
///
/// # Errors
///
/// Returns [`RunInteractionError`] when the answer list exceeds its entry
/// ceiling or an answer is empty or exceeds its byte ceiling.
pub fn question_command(
    thread_id: ThreadId,
    action: &RespondQuestionAction,
    request_id: RequestId,
) -> Result<Command, RunInteractionError> {
    RespondQuestion::new(
        request_id,
        thread_id,
        action.run_id.clone(),
        action.question_id.clone(),
        action.answers.clone(),
    )
    .map(Command::RespondQuestion)
}

/// Returns whether an answer list is an explicitly skipped question.
#[must_use]
pub const fn is_explicit_skip(answers: &[String]) -> bool {
    answers.is_empty()
}

/// How one dispatched answer settled against its correlated receipt.
///
/// Variants carry only renderer-safe text: stable copy plus validated
/// identities, never provider payload beyond the S1c row vocabulary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AnswerSettlement {
    /// The decision was recorded and delivered; the requested row settles in
    /// place through the subscription. A duplicate disposition replays the
    /// same committed intent with no second effect.
    SettledInPlace {
        /// Whether the receipt replayed an earlier commit (`Duplicate`).
        duplicate: bool,
    },
    /// The request identity was already accepted for a different intent. The
    /// originally accepted outcome stands; repeating this request never
    /// settles. A changed intent needs a freshly minted request identity.
    Conflict {
        /// Renderer-safe conflict evidence.
        message: String,
    },
    /// The named thread/run pair is not the live owning run. Nothing was
    /// stored, so the identical request (same request identity) may be
    /// retried once the owning run is live.
    RetryWhenLive {
        /// Renderer-safe retry evidence.
        message: String,
    },
    /// The answer did not settle for a retryable reason. The identical
    /// request (same request identity) may be retried explicitly.
    RetryableFailure {
        /// Renderer-safe failure evidence.
        message: String,
    },
    /// An unknown or unmatched result that settles nothing. Rendered as a
    /// diagnostic row; never a panic and never a silent settle.
    Diagnostic {
        /// Renderer-safe diagnostic evidence.
        message: String,
    },
}

impl AnswerSettlement {
    /// Returns whether the answer settled its target in place.
    #[must_use]
    pub const fn is_settled(&self) -> bool {
        matches!(self, Self::SettledInPlace { .. })
    }

    /// Returns whether the settlement replayed an earlier commit.
    #[must_use]
    pub const fn is_duplicate(&self) -> bool {
        matches!(self, Self::SettledInPlace { duplicate: true })
    }

    /// Returns the renderer-safe message for non-settled outcomes, if any.
    #[must_use]
    pub fn message(&self) -> Option<&str> {
        match self {
            Self::SettledInPlace { .. } => None,
            Self::Conflict { message }
            | Self::RetryWhenLive { message }
            | Self::RetryableFailure { message }
            | Self::Diagnostic { message } => Some(message),
        }
    }
}

/// Which S1c requested row a receipt paired onto.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PairedRow {
    /// The receipt's target is a requested row still awaiting its durable
    /// resolution observation.
    Requested,
    /// The receipt's target already resolved in place through the
    /// subscription.
    Resolved,
    /// No row carries the receipt's target identity.
    Unknown,
}

/// One receipt paired onto its S1c requested row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnswerPairing {
    /// How the dispatched answer settled.
    pub settlement: AnswerSettlement,
    /// Which S1c row the receipt paired onto.
    pub row: PairedRow,
}

impl AnswerPairing {
    /// Returns whether the answer settled its target in place.
    #[must_use]
    pub const fn is_settled(&self) -> bool {
        self.settlement.is_settled()
    }
}

/// Pairs one correlated approval receipt onto its S1c requested row.
///
/// The receipt must echo the dispatched command's request identity and
/// decision exactly; anything else degrades to [`AnswerSettlement::Diagnostic`]
/// and settles nothing. `Applied` and `AlreadyResolved` settle in place,
/// `Duplicate` replays settle idempotently, `WrongRun` reports retry-when-live,
/// and `UnknownTarget` degrades to a diagnostic row.
#[must_use]
pub fn pair_approval_answer(
    state: &EngineObservationState,
    command: &RespondApproval,
    receipt: &RespondApprovalReceipt,
) -> AnswerPairing {
    if receipt.request_id != *command.request_id()
        || receipt.thread_id != *command.thread_id()
        || receipt.run_id != *command.run_id()
        || receipt.approval_id != *command.approval_id()
        || receipt.approved != command.approved()
    {
        return AnswerPairing {
            settlement: AnswerSettlement::Diagnostic {
                message: format_mismatched_answer(AnswerKind::Approval, command.request_id()),
            },
            row: paired_approval_row(state, command),
        };
    }
    let row = paired_approval_row(state, command);
    let settlement = match receipt.outcome {
        RunInteractionOutcome::Applied | RunInteractionOutcome::AlreadyResolved => {
            AnswerSettlement::SettledInPlace {
                duplicate: receipt.disposition == ReceiptDisposition::Duplicate,
            }
        }
        RunInteractionOutcome::WrongRun => AnswerSettlement::RetryWhenLive {
            message: format!(
                "run {} is not live; retry the same answer once it is live",
                receipt.run_id.as_str()
            ),
        },
        RunInteractionOutcome::UnknownTarget => AnswerSettlement::Diagnostic {
            message: format!(
                "no pending approval carries this target on run {}; nothing was recorded",
                receipt.run_id.as_str()
            ),
        },
    };
    AnswerPairing { settlement, row }
}

/// Pairs one correlated question receipt onto its S1c requested row.
///
/// The same correlation, intent-echo, and outcome contract as
/// [`pair_approval_answer`]; an empty echoed answer list pairs an explicitly
/// skipped question.
#[must_use]
pub fn pair_question_answer(
    state: &EngineObservationState,
    command: &RespondQuestion,
    receipt: &RespondQuestionReceipt,
) -> AnswerPairing {
    if receipt.request_id != *command.request_id()
        || receipt.thread_id != *command.thread_id()
        || receipt.run_id != *command.run_id()
        || receipt.question_id != *command.question_id()
        || receipt.answers != *command.answers()
    {
        return AnswerPairing {
            settlement: AnswerSettlement::Diagnostic {
                message: format_mismatched_answer(AnswerKind::Question, command.request_id()),
            },
            row: paired_question_row(state, command),
        };
    }
    let row = paired_question_row(state, command);
    let settlement = match receipt.outcome {
        RunInteractionOutcome::Applied | RunInteractionOutcome::AlreadyResolved => {
            AnswerSettlement::SettledInPlace {
                duplicate: receipt.disposition == ReceiptDisposition::Duplicate,
            }
        }
        RunInteractionOutcome::WrongRun => AnswerSettlement::RetryWhenLive {
            message: format!(
                "run {} is not live; retry the same answer once it is live",
                receipt.run_id.as_str()
            ),
        },
        RunInteractionOutcome::UnknownTarget => AnswerSettlement::Diagnostic {
            message: format!(
                "no pending question carries this target on run {}; nothing was recorded",
                receipt.run_id.as_str()
            ),
        },
    };
    AnswerPairing { settlement, row }
}

/// Pairs one correlated answer failure onto its pending answer.
///
/// An [`ErrorCode::IdempotencyConflict`] surfaces the conflict message: the
/// originally accepted outcome stands and this request identity must never be
/// retried. A retryable failure surfaces a retryable failure for an explicit
/// same-identity retry. Any other failure degrades to a diagnostic row. A
/// failure naming another request (or none) never settles this answer.
#[must_use]
pub fn pair_answer_failure(
    kind: AnswerKind,
    request_id: &RequestId,
    failure: &ProtocolFailure,
) -> AnswerSettlement {
    if failure.request_id.as_ref() != Some(request_id) {
        return AnswerSettlement::Diagnostic {
            message: format!(
                "{}the failure names another request; nothing was recorded",
                kind.failure_prefix()
            ),
        };
    }
    if failure.code == ErrorCode::IdempotencyConflict {
        return AnswerSettlement::Conflict {
            message: format!(
                "{}{}; the originally accepted outcome stands",
                kind.failure_prefix(),
                failure.detail.as_str()
            ),
        };
    }
    if failure.retryable {
        return AnswerSettlement::RetryableFailure {
            message: format!(
                "{}the answer did not settle; retry the same answer",
                kind.failure_prefix()
            ),
        };
    }
    AnswerSettlement::Diagnostic {
        message: format!(
            "{}{}; nothing was recorded",
            kind.failure_prefix(),
            failure.detail.as_str()
        ),
    }
}

/// Returns which S1c row an approval command pairs onto.
fn paired_approval_row(state: &EngineObservationState, command: &RespondApproval) -> PairedRow {
    match state.approval(command.approval_id().as_str()) {
        None => PairedRow::Unknown,
        Some(row) if row.is_requested() => PairedRow::Requested,
        Some(_) => PairedRow::Resolved,
    }
}

/// Returns which S1c row a question command pairs onto.
fn paired_question_row(state: &EngineObservationState, command: &RespondQuestion) -> PairedRow {
    match state.question(command.question_id().as_str()) {
        None => PairedRow::Unknown,
        Some(row) if row.is_requested() => PairedRow::Requested,
        Some(_) => PairedRow::Resolved,
    }
}

/// Formats the diagnostic for a receipt that does not echo its command.
fn format_mismatched_answer(kind: AnswerKind, request_id: &RequestId) -> String {
    format!(
        "{}the recorded answer does not match request {}; nothing was applied",
        kind.failure_prefix(),
        request_id.as_str()
    )
}

/// Returns the in-flight label for a submitted approval decision.
///
/// A submitted denial always reads `Denying…`; otherwise the label follows
/// the presentation kind, matching `conversation-approval.svelte`.
#[must_use]
pub const fn pending_approval_label(kind: &ApprovalKind, submitted_denial: bool) -> &'static str {
    if submitted_denial {
        APPROVAL_DENYING_LABEL
    } else {
        match kind {
            ApprovalKind::Command => "Starting…",
            ApprovalKind::FileChange => "Applying…",
            ApprovalKind::Action | ApprovalKind::Unknown(_) => "Approving…",
        }
    }
}

/// Renderer-facing answer state for one approval row.
///
/// The presentation always comes from the shared
/// [`ApprovalPresentation`](crate::approval_presentation) policy via
/// [`ApprovalRow::presentation`]; this view only splits pending affordances
/// from settled decisions.
#[derive(Clone, Debug, PartialEq)]
pub enum ApprovalAnswerView {
    /// The row awaits a decision and exposes both explicit gestures.
    Pending {
        /// Exact renderer-facing presentation for the row.
        presentation: ApprovalPresentation,
    },
    /// The row settled in place with its recorded decision.
    Decided {
        /// The recorded decision: true allowed, false denied.
        approved: bool,
        /// Exact renderer-facing presentation for the row.
        presentation: ApprovalPresentation,
    },
}

impl ApprovalAnswerView {
    /// Returns whether the row still awaits a decision.
    #[must_use]
    pub const fn is_pending(&self) -> bool {
        matches!(self, Self::Pending { .. })
    }

    /// Returns the exact renderer-facing presentation for the row.
    #[must_use]
    pub const fn presentation(&self) -> &ApprovalPresentation {
        match self {
            Self::Pending { presentation } | Self::Decided { presentation, .. } => presentation,
        }
    }

    /// Returns the recorded decision, or [`None`] while pending.
    #[must_use]
    pub const fn decided(&self) -> Option<bool> {
        match self {
            Self::Pending { .. } => None,
            Self::Decided { approved, .. } => Some(*approved),
        }
    }
}

/// Projects one S1c approval row into its renderer-facing answer state.
#[must_use]
pub fn approval_answer_view(row: &ApprovalRow) -> ApprovalAnswerView {
    let presentation = row.presentation();
    match row.approved() {
        None => ApprovalAnswerView::Pending { presentation },
        Some(approved) => ApprovalAnswerView::Decided {
            approved,
            presentation,
        },
    }
}

/// Renderer-facing answer state for one question row.
///
/// A provider that enumerated its answers asks for a choice, not prose: any
/// offered option list renders choices honoring `multi_select`, while a
/// question without options renders free-form entry. Only the sanitized S1c
/// row vocabulary reaches these views.
#[derive(Clone, Debug, PartialEq)]
pub enum QuestionAnswerView<'row> {
    /// The row awaits answers chosen from the offered options.
    Choice {
        /// Whether more than one option may be chosen before confirming.
        multi_select: bool,
        /// The offered answers in provider order.
        options: &'row [QuestionOptionView],
    },
    /// The row awaits a typed answer; no options were offered.
    FreeForm,
    /// The row settled in place with its recorded answers.
    Answered {
        /// The recorded answers, possibly empty for a skipped question.
        answers: &'row [String],
    },
}

impl QuestionAnswerView<'_> {
    /// Returns whether the row still awaits answers.
    #[must_use]
    pub const fn is_pending(&self) -> bool {
        matches!(self, Self::Choice { .. } | Self::FreeForm)
    }
}

/// Projects one S1c question row into its renderer-facing answer state.
#[must_use]
pub fn question_answer_view(row: &QuestionRow) -> QuestionAnswerView<'_> {
    if let Some(answers) = row.answers() {
        return QuestionAnswerView::Answered { answers };
    }
    match row.options() {
        Some(options) if !options.is_empty() => QuestionAnswerView::Choice {
            multi_select: row.multi_select(),
            options,
        },
        _ => QuestionAnswerView::FreeForm,
    }
}

/// Single-flight gate for one pending answer gesture.
///
/// Mirrors `submitted_decision` in `conversation-approval.svelte`: at most
/// one answer attempt is in flight per row, so a second gesture cannot mint
/// a second request identity while the first awaits its receipt. Affordances
/// disable while [`Self::is_in_flight`] holds.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AnswerFlight {
    in_flight: bool,
}

impl AnswerFlight {
    /// Creates a gate with no answer in flight.
    #[must_use]
    pub const fn new() -> Self {
        Self { in_flight: false }
    }

    /// Returns whether an answer attempt is awaiting its receipt.
    #[must_use]
    pub const fn is_in_flight(&self) -> bool {
        self.in_flight
    }

    /// Admits one answer attempt, returning false when one is already in
    /// flight. A refused attempt mints nothing and dispatches nothing.
    #[must_use]
    pub fn begin(&mut self) -> bool {
        if self.in_flight {
            return false;
        }
        self.in_flight = true;
        true
    }

    /// Releases the gate after the attempt settles (receipt or failure).
    pub fn settle(&mut self) {
        self.in_flight = false;
    }
}
