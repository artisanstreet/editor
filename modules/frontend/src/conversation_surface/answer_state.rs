//! Answer state for approval and question rows: single-flight gates,
//! dispatch records, and the transport drain policy.

use super::*;

/// Stable debug-selector suffix for the approval confirm control.
pub const APPROVAL_CONFIRM_SELECTOR_SUFFIX: &str = "approve-submit";

/// Stable debug-selector suffix for the approval deny control.
pub const APPROVAL_DENY_SELECTOR_SUFFIX: &str = "deny-submit";

/// Stable debug-selector suffix for the question answer control.
pub const QUESTION_ANSWER_SELECTOR_SUFFIX: &str = "question-answer";

/// Stable debug-selector suffix for one question option control; the option
/// index is appended after a `-` separator.
pub const QUESTION_OPTION_SELECTOR_SUFFIX: &str = "question-option";

/// Stable debug-selector suffix for the question failure row.
pub const QUESTION_FAILURE_SELECTOR_SUFFIX: &str = "question-failure";

/// Stable debug-selector suffix for the approval failure row.
pub const APPROVAL_FAILURE_SELECTOR_SUFFIX: &str = "approval-failure";

/// Which existing GPUI answer action a dispatched attempt carries.
///
/// No new actions are introduced: this enum only retains the already
/// registered [`RespondApprovalAction`]/[`RespondQuestionAction`] value that
/// a button gesture dispatched, alongside the domain command built for it.
#[derive(Clone, Debug, PartialEq)]
pub enum AnswerDispatchAction {
    /// One explicit approval gesture (`approved` is always stated).
    Approval(RespondApprovalAction),
    /// One explicit question gesture with the chosen answers.
    Question(RespondQuestionAction),
}

impl AnswerDispatchAction {
    /// Returns the stable action name shared with the GPUI contract.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Approval(_) => "respond_approval",
            Self::Question(_) => "respond_question",
        }
    }
}

/// One button gesture dispatched toward the existing transport/request path.
///
/// The command already carries its freshly minted request identity; the
/// controller drains [`ConversationSurface::take_answer_dispatches`] toward
/// transport. App-level `dispatch_action` binding for the carried GPUI action
/// value lands with the controller transport packet; until then the outbox
/// carries the exact action values the buttons dispatched.
#[derive(Debug)]
pub struct AnswerDispatch {
    /// The existing GPUI answer action value the gesture dispatched.
    pub action: AnswerDispatchAction,
    /// The domain command built through the existing constructors.
    pub command: Command,
    /// The freshly minted request identity carried by the command.
    pub request_id: RequestId,
}

/// Offered question options cached per row for choice rendering.
///
/// The full option views remain in [`EngineObservationState`]; this cache
/// carries only the labels (plus optional descriptions) the controller
/// mirrors for the choice buttons, with the multi-select policy.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QuestionChoiceCache {
    /// Whether more than one option may be chosen before confirming.
    pub multi_select: bool,
    /// Offered answers in provider order as `(label, description)` pairs.
    pub options: Vec<(String, Option<String>)>,
}

impl QuestionChoiceCache {
    /// Returns whether the row offers choices rather than free-form entry.
    #[must_use]
    pub fn is_choice(&self) -> bool {
        !self.options.is_empty()
    }
}

/// One approval answer attempt admitted by [`ApprovalAnswerGate`].
///
/// The attempt carries the dispatched GPUI action value, the domain command
/// built with a freshly minted request identity, and that identity for
/// receipt correlation. At most one attempt exists per row while its flight
/// is outstanding.
#[derive(Debug)]
pub struct ApprovalAnswerAttempt {
    /// The dispatched `respond_approval` action value.
    pub action: RespondApprovalAction,
    /// The domain command built through [`approval_command`].
    pub command: Command,
    /// The freshly minted request identity carried by the command.
    pub request_id: RequestId,
}

/// Single-flight gate plus pending/failure presentation for one approval row.
///
/// Mirrors `submitted_decision` in `conversation-approval.svelte`: at most
/// one answer attempt is in flight per row, so a second gesture cannot mint
/// a second request identity while the first awaits its receipt. A settled
/// attempt keeps the gate closed until the row resolves in place through the
/// subscription; a failed attempt reopens the gate and surfaces the retry
/// message from the existing pairing policy.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ApprovalAnswerGate {
    pub(super) flight: AnswerFlight,
    pub(super) pending_decision: Option<bool>,
    pub(super) failure: Option<String>,
    pub(super) last_request_id: Option<RequestId>,
}

impl ApprovalAnswerGate {
    /// Creates a gate with no answer in flight.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns whether an answer attempt is awaiting its receipt.
    #[must_use]
    pub fn is_in_flight(&self) -> bool {
        self.flight.is_in_flight()
    }

    /// Returns the submitted decision awaiting settlement, if any.
    #[must_use]
    pub const fn pending_decision(&self) -> Option<bool> {
        self.pending_decision
    }

    /// Returns the surfaced retry/diagnostic message, if any.
    #[must_use]
    pub fn failure_message(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    /// Returns the request identity of the latest admitted attempt, if any.
    #[must_use]
    pub fn last_request_id(&self) -> Option<&RequestId> {
        self.last_request_id.as_ref()
    }

    /// Attempts one explicit approval gesture.
    ///
    /// This is one authenticated user gesture: `approved` is always stated,
    /// never defaulted. A refused attempt (flight outstanding or identity
    /// exhaustion) mints nothing and dispatches nothing.
    pub fn begin(
        &mut self,
        thread_id: ThreadId,
        run_id: RunId,
        approval_id: ObservationId,
        approved: bool,
    ) -> Option<ApprovalAnswerAttempt> {
        if !self.flight.begin() {
            return None;
        }
        let Ok(request_id) = mint_answer_request_id() else {
            self.flight.settle();
            return None;
        };
        let action = RespondApprovalAction {
            run_id,
            approval_id,
            approved,
        };
        let command = approval_command(thread_id, &action, request_id.clone());
        self.pending_decision = Some(approved);
        self.failure = None;
        self.last_request_id = Some(request_id.clone());
        Some(ApprovalAnswerAttempt {
            action,
            command,
            request_id,
        })
    }

    /// Pairs one correlated approval receipt onto the existing policy.
    ///
    /// No new pairing logic is introduced: this delegates to
    /// [`pair_approval_answer`]. A settled attempt keeps the gate closed
    /// until the row resolves in place; any other outcome reopens the gate
    /// and surfaces the renderer-safe message.
    pub fn settle_receipt(
        &mut self,
        state: &EngineObservationState,
        command: &artisan_domain::RespondApproval,
        receipt: &RespondApprovalReceipt,
    ) -> AnswerPairing {
        let pairing = pair_approval_answer(state, command, receipt);
        if pairing.is_settled() {
            self.failure = None;
        } else {
            self.flight.settle();
            self.pending_decision = None;
            self.failure = pairing.settlement.message().map(str::to_owned);
        }
        pairing
    }

    /// Pairs one answer failure onto the existing policy.
    ///
    /// The retry message comes from [`pair_answer_failure`]; the gate
    /// reopens so the same answer may be retried explicitly with a freshly
    /// minted identity.
    pub fn settle_failure(
        &mut self,
        request_id: &RequestId,
        failure: &ProtocolFailure,
    ) -> AnswerSettlement {
        let settlement = pair_answer_failure(AnswerKind::Approval, request_id, failure);
        self.flight.settle();
        self.pending_decision = None;
        self.failure = settlement.message().map(str::to_owned);
        settlement
    }
}

/// What one question-row keystroke did.
///
/// Returned by [`ConversationSurface::handle_question_key`] so renderers can
/// route focus without re-deriving the decision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuestionKeyOutcome {
    /// The key changed nothing: wrong row kind, outstanding flight,
    /// modified shortcut, unhandled key, or a refused intake/submit.
    Ignored,
    /// Typed or deleted text staged into the row draft.
    Edited,
    /// `enter` dispatched the staged draft through the existing question
    /// submit gesture with a fresh request id.
    Submitted,
    /// `escape` asks the renderer to return focus to the transcript without
    /// submitting.
    FocusTranscript,
}

/// Shapes raw staged text into the single-line free-form draft form.
///
/// Canonicalizes through the shared input intake, then strips newlines for
/// legacy single-line `Input` parity (the browser input drops them; Enter
/// submits instead of inserting a break here).
pub(super) fn shape_freeform_draft(text: &str) -> String {
    artisan_ui::input_state::normalize_input(text)
        .chars()
        .filter(|character| *character != '\n')
        .collect()
}

/// One question answer attempt admitted by [`QuestionAnswerGate`].
///
/// Carries the dispatched `respond_question` action value, the domain command
/// built with a freshly minted request identity, and that identity for
/// receipt correlation.
#[derive(Debug)]
pub struct QuestionAnswerAttempt {
    /// The dispatched `respond_question` action value.
    pub action: RespondQuestionAction,
    /// The domain command built through [`question_command`].
    pub command: Command,
    /// The freshly minted request identity carried by the command.
    pub request_id: RequestId,
}

/// Single-flight gate plus selection/draft/failure state for one question row.
///
/// Mirrors `conversation-prompt.svelte`: a single-select choice submits the
/// moment it is clicked, while a multi-select choice stages a selection until
/// the Answer control confirms it. Free-form rows submit the typed draft;
/// empty drafts are rejected client-side with the row staying pending, so
/// nothing is minted or dispatched. A settled attempt keeps the gate closed
/// until the row resolves; a failed attempt reopens it with the retry message
/// from the existing pairing policy.
///
/// The free-form draft is backed by [`TextInputState`](artisan_ui::input_state::TextInputState):
/// keystroke intake is canonicalized on entry (zero-width spaces removed,
/// CR/CRLF folded) exactly like the shared input seam, while the
/// single-line caller policy additionally strips newlines (legacy `Input`
/// parity: the browser single-line input drops them) and clamps the buffer
/// to [`OBSERVATION_ANSWER_MAX_BYTES`] UTF-8 bytes, the same bound the
/// answer validation enforces at submit. `TextInputState` carries no
/// `PartialEq`, so this gate deliberately omits it; no caller compares
/// gates.
#[derive(Clone, Debug, Default)]
pub struct QuestionAnswerGate {
    pub(super) flight: AnswerFlight,
    pub(super) selected: Vec<String>,
    pub(super) draft: TextInputState,
    pub(super) failure: Option<String>,
    pub(super) last_request_id: Option<RequestId>,
}

impl QuestionAnswerGate {
    /// Creates a gate with no selection, draft, or flight.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns whether an answer attempt is awaiting its receipt.
    #[must_use]
    pub fn is_in_flight(&self) -> bool {
        self.flight.is_in_flight()
    }

    /// Returns the staged multi-select choices in selection order.
    #[must_use]
    pub fn selected(&self) -> &[String] {
        &self.selected
    }

    /// Returns the staged free-form draft in canonical input form.
    #[must_use]
    pub fn draft(&self) -> &str {
        self.draft.value()
    }

    /// Returns the surfaced retry/diagnostic message, if any.
    #[must_use]
    pub fn failure_message(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    /// Returns the request identity of the latest admitted attempt, if any.
    #[must_use]
    pub fn last_request_id(&self) -> Option<&RequestId> {
        self.last_request_id.as_ref()
    }

    /// Stages one multi-select choice toggle without dispatching.
    ///
    /// Single-select rows never stage: they submit immediately through
    /// [`Self::submit_single`].
    pub fn toggle_option(&mut self, option: String, multi_select: bool) {
        if !multi_select {
            return;
        }
        if let Some(position) = self.selected.iter().position(|known| known == &option) {
            self.selected.remove(position);
        } else {
            self.selected.push(option);
        }
    }

    /// Replaces the staged free-form draft exactly as supplied.
    ///
    /// The text passes through the single-line shaping (canonical input
    /// form, newlines stripped); the byte bound is enforced at keystroke
    /// intake and at submit validation, not here, so controller-staged
    /// drafts arrive intact for the existing submit path to judge.
    pub fn set_draft(&mut self, draft: &str) {
        self.draft.set_value(&shape_freeform_draft(draft));
    }

    /// Appends keystroke text to the staged free-form draft.
    ///
    /// Returns whether the draft changed: empty text and appends that would
    /// exceed [`OBSERVATION_ANSWER_MAX_BYTES`] UTF-8 bytes are refused with
    /// the buffer untouched, so the draft stays within the bound the answer
    /// validation enforces.
    #[must_use]
    pub fn insert_text(&mut self, text: &str) -> bool {
        if text.is_empty() {
            return false;
        }
        let mut next = self.draft.value().to_owned();
        next.push_str(text);
        let next = shape_freeform_draft(&next);
        if next.len() > OBSERVATION_ANSWER_MAX_BYTES {
            return false;
        }
        self.draft.set_value(&next)
    }

    /// Deletes the last character of the staged free-form draft.
    ///
    /// Returns whether the draft changed; an already-empty draft reports
    /// `false`. There is no caret or selection model, matching the shared
    /// input seam limits.
    #[must_use]
    pub fn delete_backward(&mut self) -> bool {
        let mut next = self.draft.value().to_owned();
        if next.pop().is_none() {
            return false;
        }
        self.draft.set_value(&next);
        true
    }

    /// Submits one single-select choice immediately.
    ///
    /// A refused attempt (flight outstanding or identity/bounds failure)
    /// mints nothing and dispatches nothing.
    pub fn submit_single(
        &mut self,
        thread_id: ThreadId,
        run_id: RunId,
        question_id: ObservationId,
        option: String,
    ) -> Option<QuestionAnswerAttempt> {
        self.submit_answers(thread_id, run_id, question_id, vec![option])
    }

    /// Submits the staged multi-select choices.
    ///
    /// An empty selection is rejected client-side: the row stays pending and
    /// nothing is minted or dispatched.
    pub fn submit_selected(
        &mut self,
        thread_id: ThreadId,
        run_id: RunId,
        question_id: ObservationId,
    ) -> Option<QuestionAnswerAttempt> {
        let answers = self.selected.clone();
        self.submit_answers(thread_id, run_id, question_id, answers)
    }

    /// Submits an explicit choice list without touching staged state.
    ///
    /// An empty list is rejected client-side, mirroring the legacy guard
    /// that never synthesizes an answer; the empty list remains reserved for
    /// an explicit skip gesture, which has no button on this surface.
    pub fn submit_choice(
        &mut self,
        thread_id: ThreadId,
        run_id: RunId,
        question_id: ObservationId,
        answers: Vec<String>,
    ) -> Option<QuestionAnswerAttempt> {
        self.submit_answers(thread_id, run_id, question_id, answers)
    }

    /// Submits the staged free-form draft.
    ///
    /// The draft is trimmed and empty submits are rejected client-side with
    /// the row staying pending: no identity is minted and nothing is
    /// dispatched. A submitted draft clears the staged draft and selection,
    /// mirroring the legacy surface.
    pub fn submit_freeform(
        &mut self,
        thread_id: ThreadId,
        run_id: RunId,
        question_id: ObservationId,
    ) -> Option<QuestionAnswerAttempt> {
        let trimmed = self.draft.value().trim().to_owned();
        if trimmed.is_empty() {
            return None;
        }
        let attempt = self.submit_answers(thread_id, run_id, question_id, vec![trimmed])?;
        self.draft.set_value("");
        self.selected.clear();
        Some(attempt)
    }

    fn submit_answers(
        &mut self,
        thread_id: ThreadId,
        run_id: RunId,
        question_id: ObservationId,
        answers: Vec<String>,
    ) -> Option<QuestionAnswerAttempt> {
        if answers.is_empty() {
            return None;
        }
        if !self.flight.begin() {
            return None;
        }
        let Ok(request_id) = mint_answer_request_id() else {
            self.flight.settle();
            return None;
        };
        let action = RespondQuestionAction {
            run_id,
            question_id,
            answers,
        };
        let command = match question_command(thread_id, &action, request_id.clone()) {
            Ok(command) => command,
            Err(error) => {
                self.flight.settle();
                self.failure = Some(format!("{}{error}", AnswerKind::Question.failure_prefix()));
                return None;
            }
        };
        self.failure = None;
        self.last_request_id = Some(request_id.clone());
        Some(QuestionAnswerAttempt {
            action,
            command,
            request_id,
        })
    }

    /// Pairs one correlated question receipt onto the existing policy.
    ///
    /// No new pairing logic is introduced: this delegates to
    /// [`pair_question_answer`]. A settled attempt keeps the gate closed
    /// until the row resolves in place; any other outcome reopens the gate
    /// and surfaces the renderer-safe message.
    pub fn settle_receipt(
        &mut self,
        state: &EngineObservationState,
        command: &artisan_domain::RespondQuestion,
        receipt: &RespondQuestionReceipt,
    ) -> AnswerPairing {
        let pairing = pair_question_answer(state, command, receipt);
        if pairing.is_settled() {
            self.failure = None;
            self.draft.set_value("");
            self.selected.clear();
        } else {
            self.flight.settle();
            self.failure = pairing.settlement.message().map(str::to_owned);
        }
        pairing
    }

    /// Pairs one answer failure onto the existing policy.
    ///
    /// The retry message comes from [`pair_answer_failure`]; the gate
    /// reopens so the same answer may be retried explicitly with a freshly
    /// minted identity.
    pub fn settle_failure(
        &mut self,
        request_id: &RequestId,
        failure: &ProtocolFailure,
    ) -> AnswerSettlement {
        let settlement = pair_answer_failure(AnswerKind::Question, request_id, failure);
        self.flight.settle();
        self.failure = settlement.message().map(str::to_owned);
        settlement
    }
}

/// One answer dispatch that a drain attempt could not hand to transport.
///
/// The dispatch is re-queued in the surface outbox so nothing is silently
/// dropped. Its row stays pending (the gate remains in flight until a receipt
/// pairs through the existing settle-in-place pairing) and `message` carries
/// the existing retry/diagnostic text from [`pair_answer_failure`].
#[derive(Clone, Debug, PartialEq)]
pub struct FailedAnswerDispatch {
    /// The already-minted request identity of the unsent dispatch.
    pub request_id: RequestId,
    /// The existing retry/diagnostic message for the send failure.
    pub message: String,
}

/// Finite report of one outbox drain call.
///
/// Each taken dispatch is accounted exactly once: either sent or re-queued
/// with its retry message. A second drain over an emptied outbox reports
/// zeros without touching the send path.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AnswerDrainReport {
    /// Dispatches handed to the send path in this call.
    pub sent: usize,
    /// Dispatches the send path refused, re-queued with retry messages.
    pub failed: Vec<FailedAnswerDispatch>,
}

impl AnswerDrainReport {
    /// Returns whether the drain call moved every taken dispatch.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.failed.is_empty()
    }
}

/// Maps one mirrored send-path failure onto the existing answer retry policy.
///
/// [`CommandSendError::Busy`] (bounded queue full, mirroring the composer
/// backpressure path) becomes a retryable failure so the report carries the
/// existing "retry the same answer" text; [`CommandSendError::Stopped`]
/// (service gone) becomes a terminal failure so the report carries the
/// existing diagnostic text. No new pairing logic is introduced: the message
/// always comes from [`pair_answer_failure`].
pub(super) fn answer_send_failure(
    _kind: AnswerKind,
    request_id: &RequestId,
    error: CommandSendError,
) -> ProtocolFailure {
    let (detail, retryable) = match error {
        CommandSendError::Busy => ("answer transport is busy", true),
        CommandSendError::Stopped => ("answer transport has stopped", false),
    };
    ProtocolFailure {
        code: ErrorCode::Internal,
        detail: ErrorDetail::parse(detail).unwrap_or_default(),
        retryable,
        request_id: Some(request_id.clone()),
    }
}

/// Maps one taken answer dispatch onto its live transport command.
///
/// The domain command moves across unchanged with its already-minted request
/// id (cloned once for the by-value transport wrapper, exactly like the
/// `StopRun` arm clones; never re-minted). The outbox carries only
/// gate-built answer commands, so any other command is unreachable.
pub(super) fn answer_transport_command(dispatch: &AnswerDispatch) -> NativeTransportCommand {
    match &dispatch.command {
        Command::RespondApproval(answer) => {
            NativeTransportCommand::RespondApproval(Box::new(answer.clone()))
        }
        Command::RespondQuestion(answer) => {
            NativeTransportCommand::RespondQuestion(Box::new(answer.clone()))
        }
        _ => unreachable!("the answer outbox carries only answer commands"),
    }
}

/// Drains one taken answer queue through the transport submit path.
///
/// Every dispatch is mapped through [`answer_transport_command`] and handed
/// to `submit` at most once, in FIFO order; the queue itself is consumed,
/// never re-taken or cloned. Refused dispatches are returned for re-queue
/// with the existing retry message from [`pair_answer_failure`]; an empty
/// queue never touches `submit`. Row gates are never settled here:
/// single-flight is preserved until resolutions pair through the existing
/// receipt path.
///
/// # Panics
///
/// Panics if a refused dispatch unexpectedly pairs as a settled settlement;
/// [`pair_answer_failure`] is required to return an unsettled failure pair.
pub fn drain_answer_queue(
    queue: Vec<AnswerDispatch>,
    submit: &mut impl FnMut(NativeTransportCommand) -> Result<(), CommandSendError>,
) -> (Vec<AnswerDispatch>, AnswerDrainReport) {
    let mut requeue = Vec::with_capacity(queue.len());
    let mut report = AnswerDrainReport::default();
    for dispatch in queue {
        match submit(answer_transport_command(&dispatch)) {
            Ok(()) => {
                report.sent = report.sent.saturating_add(1);
            }
            Err(error) => {
                let kind = match dispatch.action {
                    AnswerDispatchAction::Approval(_) => AnswerKind::Approval,
                    AnswerDispatchAction::Question(_) => AnswerKind::Question,
                };
                let failure = answer_send_failure(kind, &dispatch.request_id, error);
                let settlement = pair_answer_failure(kind, &dispatch.request_id, &failure);
                let message = settlement
                    .message()
                    .map_or_else(|| failure.detail.as_str().to_owned(), str::to_owned);
                report.failed.push(FailedAnswerDispatch {
                    request_id: dispatch.request_id.clone(),
                    message,
                });
                requeue.push(dispatch);
            }
        }
    }
    (requeue, report)
}
