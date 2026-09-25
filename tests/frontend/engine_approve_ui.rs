//! Finite A-approve UI: GPUI answer actions plus requested-to-resolved pairing.
//!
//! These tests run under the existing cargo frontend `[[test]]` harness, the
//! same harness the current frontend unit tests use: plain `#[test]`
//! functions against the production `artisan_frontend`, `artisan_domain`,
//! and `artisan_protocol` APIs, with no build-tool-specific or display dependencies.

use artisan_domain::{
    ApprovalObservation, ApprovalRequest, Command, EngineObservationEvent, Observation,
    ObservationId, ObservationSequence, QuestionInput, QuestionObservation, QuestionOption,
    ReceiptDisposition, RequestId, RunId, ThreadId,
};
use artisan_frontend::engine_approve_ui::{
    AnswerFlight, AnswerKind, AnswerSettlement, ApprovalAnswerView, PairedRow, QuestionAnswerView,
    approval_answer_view, approval_command, is_explicit_skip, mint_answer_request_id,
    pair_answer_failure, pair_approval_answer, pair_question_answer, pending_approval_label,
    question_answer_view, question_command,
};
use artisan_frontend::engine_observation_state::EngineObservationState;
use artisan_protocol::{
    ErrorCode, ErrorDetail, ProtocolFailure, RespondApprovalReceipt, RespondQuestionReceipt,
    RunInteractionOutcome,
};
use gpui::Action as _;

fn observation_id(value: &str) -> ObservationId {
    ObservationId::parse(value).expect("fixture observation id is valid")
}

fn sequence(value: u64) -> ObservationSequence {
    ObservationSequence::new(value).expect("fixture sequence is valid")
}

fn thread_id() -> ThreadId {
    ThreadId::parse("thread-approve").expect("fixture thread id is valid")
}

fn run_id() -> RunId {
    RunId::parse("run-approve").expect("fixture run id is valid")
}

fn request_id(value: &str) -> RequestId {
    RequestId::parse(value).expect("fixture request id is valid")
}

fn event(observation: Observation) -> EngineObservationEvent {
    EngineObservationEvent {
        thread_id: thread_id(),
        observation,
        attribution: None,
    }
}

fn approval_requested(approval: &str) -> Observation {
    Observation::Approval(
        ApprovalObservation::requested(
            observation_id(&format!("obs-{approval}-requested")),
            sequence(9),
            observation_id(approval),
            String::from("Run the test suite?"),
            ApprovalRequest::command(
                String::from("cargo test"),
                Some(String::from("C:/repos/demo")),
                Some(String::from("verify before landing")),
            )
            .expect("fixture approval request is valid"),
        )
        .expect("fixture requested approval is valid"),
    )
}

fn approval_resolved(approval: &str, approved: bool) -> Observation {
    Observation::Approval(
        ApprovalObservation::resolved(
            observation_id(&format!("obs-{approval}-resolved")),
            sequence(10),
            observation_id(approval),
            String::from("Run the test suite?"),
            ApprovalRequest::command(
                String::from("cargo test"),
                Some(String::from("C:/repos/demo")),
                Some(String::from("verify before landing")),
            )
            .expect("fixture approval request is valid"),
            approved,
        )
        .expect("fixture resolved approval is valid"),
    )
}

fn question_input(
    question: &str,
    multi_select: bool,
    options: Option<Vec<QuestionOption>>,
) -> QuestionInput {
    QuestionInput {
        question_id: observation_id(question),
        text: String::from("Which runtime?"),
        header: Some(String::from("Runtime")),
        multi_select,
        options,
    }
}

fn first_options() -> Vec<QuestionOption> {
    vec![
        QuestionOption::new(String::from("tokio"), None).expect("fixture option is valid"),
        QuestionOption::new(
            String::from("async-std"),
            Some(String::from("alternative runtime")),
        )
        .expect("fixture option is valid"),
    ]
}

fn question_requested(
    question: &str,
    multi_select: bool,
    options: Option<Vec<QuestionOption>>,
) -> Observation {
    Observation::Question(
        QuestionObservation::requested(
            observation_id(&format!("obs-{question}-requested")),
            sequence(11),
            question_input(question, multi_select, options),
        )
        .expect("fixture requested question is valid"),
    )
}

fn question_resolved(question: &str, answers: Vec<String>) -> Observation {
    Observation::Question(
        QuestionObservation::resolved(
            observation_id(&format!("obs-{question}-resolved")),
            sequence(12),
            question_input(question, false, Some(first_options())),
            answers,
        )
        .expect("fixture resolved question is valid"),
    )
}

fn approval_action(
    approval: &str,
    approved: bool,
) -> artisan_frontend::engine_approve_ui::RespondApprovalAction {
    artisan_frontend::engine_approve_ui::RespondApprovalAction {
        run_id: run_id(),
        approval_id: observation_id(approval),
        approved,
    }
}

fn question_action(
    question: &str,
    answers: Vec<String>,
) -> artisan_frontend::engine_approve_ui::RespondQuestionAction {
    artisan_frontend::engine_approve_ui::RespondQuestionAction {
        run_id: run_id(),
        question_id: observation_id(question),
        answers,
    }
}

fn approval_receipt(
    request: &str,
    approval: &str,
    approved: bool,
    outcome: RunInteractionOutcome,
    disposition: ReceiptDisposition,
) -> RespondApprovalReceipt {
    RespondApprovalReceipt {
        request_id: request_id(request),
        thread_id: thread_id(),
        run_id: run_id(),
        approval_id: observation_id(approval),
        approved,
        outcome,
        disposition,
    }
}

fn question_receipt(
    request: &str,
    question: &str,
    answers: Vec<String>,
    outcome: RunInteractionOutcome,
    disposition: ReceiptDisposition,
) -> RespondQuestionReceipt {
    RespondQuestionReceipt {
        request_id: request_id(request),
        thread_id: thread_id(),
        run_id: run_id(),
        question_id: observation_id(question),
        answers,
        outcome,
        disposition,
    }
}

fn failure(code: ErrorCode, retryable: bool, request: Option<&str>) -> ProtocolFailure {
    ProtocolFailure {
        code,
        detail: ErrorDetail::parse("fixture failure evidence").expect("fixture detail is valid"),
        retryable,
        request_id: request.map(request_id),
    }
}

fn approval_command_for(
    approval: &str,
    approved: bool,
    request: &str,
) -> artisan_domain::RespondApproval {
    let command = approval_command(
        thread_id(),
        &approval_action(approval, approved),
        request_id(request),
    );
    match command {
        Command::RespondApproval(command) => command,
        Command::RespondQuestion(_)
        | Command::AttachProject(_)
        | Command::CreateThread(_)
        | Command::QueueFirstMessage(_)
        | Command::QueueMessage(_)
        | Command::StopRun(_)
        | Command::SetModelFavorite(_)
        | Command::WithdrawQueuedMessage(_)
        | Command::SetThreadEngineConfig(_)
        | Command::SaveComposerDraft(_)
        | Command::UploadComposerAttachment(_)
        | Command::QueueStoredMessage(_)
        | Command::RetryFailedMessage(_)
        | Command::RecoverFailedMessage(_)
        | Command::SubmitComposerDraft(_) => {
            panic!("approval gesture must build an approval command")
        }
    }
}

fn question_command_for(
    question: &str,
    answers: Vec<String>,
    request: &str,
) -> artisan_domain::RespondQuestion {
    let command = question_command(
        thread_id(),
        &question_action(question, answers),
        request_id(request),
    )
    .expect("fixture question answers are valid");
    match command {
        Command::RespondQuestion(command) => command,
        Command::RespondApproval(_)
        | Command::AttachProject(_)
        | Command::CreateThread(_)
        | Command::QueueFirstMessage(_)
        | Command::QueueMessage(_)
        | Command::StopRun(_)
        | Command::SetModelFavorite(_)
        | Command::WithdrawQueuedMessage(_)
        | Command::SetThreadEngineConfig(_)
        | Command::SaveComposerDraft(_)
        | Command::UploadComposerAttachment(_)
        | Command::QueueStoredMessage(_)
        | Command::RetryFailedMessage(_)
        | Command::RecoverFailedMessage(_)
        | Command::SubmitComposerDraft(_) => {
            panic!("question gesture must build a question command")
        }
    }
}

#[test]
fn answer_actions_register_under_their_contract_names() {
    assert_eq!(
        artisan_frontend::engine_approve_ui::RespondApprovalAction::name_for_type(),
        "respond_approval"
    );
    assert_eq!(
        artisan_frontend::engine_approve_ui::RespondQuestionAction::name_for_type(),
        "respond_question"
    );
}

#[test]
fn approval_gesture_builds_a_command_carrying_every_identity() {
    let command = approval_command_for("approval-1", true, "answer-1");
    assert_eq!(command.request_id(), &request_id("answer-1"));
    assert_eq!(command.thread_id(), &thread_id());
    assert_eq!(command.run_id(), &run_id());
    assert_eq!(command.approval_id().as_str(), "approval-1");
    assert!(command.approved());
}

#[test]
fn approval_denial_is_an_explicit_decision_never_a_default() {
    let denied = approval_command_for("approval-1", false, "answer-deny");
    assert!(!denied.approved());
    let allowed = approval_command_for("approval-1", true, "answer-allow");
    assert!(allowed.approved());
    assert_ne!(denied.request_id(), allowed.request_id());
}

#[test]
fn question_gesture_carries_answers_and_marks_an_explicit_skip() {
    let answers = vec![String::from("tokio"), String::from("async-std")];
    let command = question_command_for("question-1", answers.clone(), "answer-2");
    assert_eq!(command.request_id(), &request_id("answer-2"));
    assert_eq!(command.thread_id(), &thread_id());
    assert_eq!(command.run_id(), &run_id());
    assert_eq!(command.question_id().as_str(), "question-1");
    assert_eq!(command.answers(), &answers);
    assert!(!is_explicit_skip(command.answers()));

    let skipped = question_command_for("question-1", Vec::new(), "answer-skip");
    assert!(is_explicit_skip(skipped.answers()));
}

#[test]
fn question_gesture_rejects_invalid_answers_instead_of_synthesising() {
    let invalid = question_command(
        thread_id(),
        &question_action("question-1", vec![String::new()]),
        request_id("answer-bad"),
    );
    assert!(matches!(
        invalid,
        Err(artisan_domain::RunInteractionError::EmptyAnswer { index: 0 })
    ));
}

#[test]
fn every_answer_attempt_mints_a_fresh_request_identity() {
    let first = mint_answer_request_id().expect("first answer identity mints");
    let second = mint_answer_request_id().expect("second answer identity mints");
    assert_ne!(first, second);
}

#[test]
fn applied_receipt_settles_a_requested_row_in_place() {
    let mut state = EngineObservationState::new(thread_id());
    state.apply(1, &event(approval_requested("approval-1")));
    let command = approval_command_for("approval-1", true, "answer-1");
    let receipt = approval_receipt(
        "answer-1",
        "approval-1",
        true,
        RunInteractionOutcome::Applied,
        ReceiptDisposition::Accepted,
    );
    let pairing = pair_approval_answer(&state, &command, &receipt);
    assert_eq!(
        pairing.settlement,
        AnswerSettlement::SettledInPlace { duplicate: false }
    );
    assert!(pairing.is_settled());
    assert_eq!(pairing.row, PairedRow::Requested);

    state.apply(2, &event(approval_resolved("approval-1", true)));
    let pairing = pair_approval_answer(&state, &command, &receipt);
    assert!(pairing.is_settled());
    assert_eq!(pairing.row, PairedRow::Resolved);
    assert_eq!(state.approvals_in_order().len(), 1);
}

#[test]
fn duplicate_replays_settle_idempotently_without_a_second_effect() {
    let mut state = EngineObservationState::new(thread_id());
    state.apply(1, &event(approval_requested("approval-1")));
    state.apply(2, &event(approval_resolved("approval-1", true)));
    let command = approval_command_for("approval-1", true, "answer-1");
    let replay = approval_receipt(
        "answer-1",
        "approval-1",
        true,
        RunInteractionOutcome::Applied,
        ReceiptDisposition::Duplicate,
    );
    let first = pair_approval_answer(&state, &command, &replay);
    let second = pair_approval_answer(&state, &command, &replay);
    assert_eq!(
        first.settlement,
        AnswerSettlement::SettledInPlace { duplicate: true }
    );
    assert_eq!(first, second);
    assert!(first.settlement.is_duplicate());
    assert_eq!(state.approvals_in_order().len(), 1);
    assert_eq!(
        state.approval("approval-1").expect("row pairs").approved(),
        Some(true)
    );
}

#[test]
fn already_resolved_receipts_settle_in_place() {
    let mut state = EngineObservationState::new(thread_id());
    state.apply(1, &event(approval_requested("approval-1")));
    state.apply(2, &event(approval_resolved("approval-1", false)));
    let command = approval_command_for("approval-1", false, "answer-3");
    let receipt = approval_receipt(
        "answer-3",
        "approval-1",
        false,
        RunInteractionOutcome::AlreadyResolved,
        ReceiptDisposition::Accepted,
    );
    let pairing = pair_approval_answer(&state, &command, &receipt);
    assert_eq!(
        pairing.settlement,
        AnswerSettlement::SettledInPlace { duplicate: false }
    );
    assert_eq!(pairing.row, PairedRow::Resolved);
}

#[test]
fn mismatched_receipts_never_settle_and_degrade_to_a_diagnostic() {
    let mut state = EngineObservationState::new(thread_id());
    state.apply(1, &event(approval_requested("approval-1")));
    let command = approval_command_for("approval-1", true, "answer-1");

    let foreign = approval_receipt(
        "answer-other",
        "approval-1",
        true,
        RunInteractionOutcome::Applied,
        ReceiptDisposition::Accepted,
    );
    let pairing = pair_approval_answer(&state, &command, &foreign);
    assert!(matches!(
        pairing.settlement,
        AnswerSettlement::Diagnostic { .. }
    ));
    assert!(!pairing.is_settled());

    let flipped = approval_receipt(
        "answer-1",
        "approval-1",
        false,
        RunInteractionOutcome::Applied,
        ReceiptDisposition::Accepted,
    );
    let pairing = pair_approval_answer(&state, &command, &flipped);
    assert!(matches!(
        pairing.settlement,
        AnswerSettlement::Diagnostic { .. }
    ));
}

#[test]
fn conflict_surfaces_the_conflict_message_and_never_retries() {
    let detail = "request `answer-1` was already accepted for a different response";
    let conflict = ProtocolFailure {
        code: ErrorCode::IdempotencyConflict,
        detail: ErrorDetail::parse(detail).expect("fixture detail is valid"),
        retryable: false,
        request_id: Some(request_id("answer-1")),
    };
    let settlement = pair_answer_failure(AnswerKind::Approval, &request_id("answer-1"), &conflict);
    match &settlement {
        AnswerSettlement::Conflict { message } => {
            assert!(message.contains(detail));
            assert!(message.contains("originally accepted outcome stands"));
        }
        AnswerSettlement::SettledInPlace { .. }
        | AnswerSettlement::RetryWhenLive { .. }
        | AnswerSettlement::RetryableFailure { .. }
        | AnswerSettlement::Diagnostic { .. } => panic!("conflict must surface"),
    }
    assert!(!settlement.is_settled());
    assert!(settlement.message().is_some());
}

#[test]
fn wrong_run_surfaces_a_retry_when_live_message() {
    let mut state = EngineObservationState::new(thread_id());
    state.apply(
        1,
        &event(question_requested(
            "question-1",
            false,
            Some(first_options()),
        )),
    );
    let answers = vec![String::from("tokio")];
    let command = question_command_for("question-1", answers.clone(), "answer-4");
    let receipt = question_receipt(
        "answer-4",
        "question-1",
        answers,
        RunInteractionOutcome::WrongRun,
        ReceiptDisposition::Accepted,
    );
    let pairing = pair_question_answer(&state, &command, &receipt);
    match &pairing.settlement {
        AnswerSettlement::RetryWhenLive { message } => {
            assert!(message.contains("not live"));
        }
        AnswerSettlement::SettledInPlace { .. }
        | AnswerSettlement::Conflict { .. }
        | AnswerSettlement::RetryableFailure { .. }
        | AnswerSettlement::Diagnostic { .. } => panic!("wrong run must ask for a live retry"),
    }
    assert_eq!(pairing.row, PairedRow::Requested);
}

#[test]
fn unavailable_failures_surface_a_retryable_failure() {
    let unavailable = failure(ErrorCode::Internal, true, Some("answer-5"));
    let settlement =
        pair_answer_failure(AnswerKind::Question, &request_id("answer-5"), &unavailable);
    match &settlement {
        AnswerSettlement::RetryableFailure { message } => {
            assert!(message.contains("retry the same answer"));
        }
        AnswerSettlement::SettledInPlace { .. }
        | AnswerSettlement::Conflict { .. }
        | AnswerSettlement::RetryWhenLive { .. }
        | AnswerSettlement::Diagnostic { .. } => {
            panic!("unavailable work must stay retryable")
        }
    }
}

#[test]
fn unknown_targets_and_foreign_failures_degrade_to_diagnostics() {
    let mut state = EngineObservationState::new(thread_id());
    state.apply(
        1,
        &event(question_requested(
            "question-1",
            false,
            Some(first_options()),
        )),
    );
    let answers = vec![String::from("tokio")];
    let command = question_command_for("question-1", answers.clone(), "answer-6");
    let receipt = question_receipt(
        "answer-6",
        "question-1",
        answers,
        RunInteractionOutcome::UnknownTarget,
        ReceiptDisposition::Accepted,
    );
    let pairing = pair_question_answer(&state, &command, &receipt);
    assert!(matches!(
        pairing.settlement,
        AnswerSettlement::Diagnostic { .. }
    ));

    let foreign = failure(ErrorCode::Internal, true, Some("answer-other"));
    let settlement = pair_answer_failure(AnswerKind::Question, &request_id("answer-6"), &foreign);
    assert!(matches!(settlement, AnswerSettlement::Diagnostic { .. }));

    let terminal = failure(ErrorCode::InvalidInput, false, Some("answer-6"));
    let settlement = pair_answer_failure(AnswerKind::Question, &request_id("answer-6"), &terminal);
    assert!(matches!(settlement, AnswerSettlement::Diagnostic { .. }));
}

#[test]
fn question_views_split_multi_select_choice_and_free_form() {
    let mut state = EngineObservationState::new(thread_id());
    state.apply(
        1,
        &event(question_requested(
            "question-multi",
            true,
            Some(first_options()),
        )),
    );
    state.apply(
        2,
        &event(question_requested(
            "question-single",
            false,
            Some(first_options()),
        )),
    );
    state.apply(3, &event(question_requested("question-free", false, None)));

    let row = state.question("question-multi").expect("multi row pairs");
    let multi_view = question_answer_view(row);
    assert!(multi_view.is_pending());
    match multi_view {
        QuestionAnswerView::Choice {
            multi_select,
            options,
        } => {
            assert!(multi_select);
            assert_eq!(options.len(), 2);
            assert_eq!(options[0].label(), "tokio");
            assert_eq!(options[1].description(), Some("alternative runtime"));
        }
        QuestionAnswerView::FreeForm | QuestionAnswerView::Answered { .. } => {
            panic!("multi-select must render choices")
        }
    }

    let row = state.question("question-single").expect("single row pairs");
    match question_answer_view(row) {
        QuestionAnswerView::Choice {
            multi_select,
            options,
        } => {
            assert!(!multi_select);
            assert_eq!(options.len(), 2);
        }
        QuestionAnswerView::FreeForm | QuestionAnswerView::Answered { .. } => {
            panic!("single-select must render choices")
        }
    }

    let row = state
        .question("question-free")
        .expect("free-form row pairs");
    assert!(question_answer_view(row).is_pending());
    assert!(matches!(
        question_answer_view(row),
        QuestionAnswerView::FreeForm
    ));

    state.apply(
        4,
        &event(question_resolved(
            "question-free",
            vec![String::from("typed")],
        )),
    );
    let row = state.question("question-free").expect("answered row pairs");
    let answered_view = question_answer_view(row);
    assert!(!answered_view.is_pending());
    match answered_view {
        QuestionAnswerView::Answered { answers } => {
            assert_eq!(answers, &[String::from("typed")]);
        }
        QuestionAnswerView::Choice { .. } | QuestionAnswerView::FreeForm => {
            panic!("resolved questions must render answers")
        }
    }
}

#[test]
fn approval_deny_then_allow_flows_settle_each_row_in_place() {
    let mut state = EngineObservationState::new(thread_id());
    state.apply(1, &event(approval_requested("approval-1")));

    let pending = state.approval("approval-1").expect("requested row pairs");
    let pending_view = approval_answer_view(pending);
    assert!(pending_view.is_pending());
    assert_eq!(pending_view.decided(), None);
    match &pending_view {
        ApprovalAnswerView::Pending { presentation } => {
            assert_eq!(presentation.approve_label, "Run command");
            assert_eq!(presentation.title, "Run this command?");
            assert_eq!(pending_view.presentation().title, "Run this command?");
        }
        ApprovalAnswerView::Decided { .. } => panic!("requested rows render pending"),
    }
    assert_eq!(
        pending_approval_label(
            &artisan_frontend::approval_presentation::ApprovalKind::Command,
            false
        ),
        "Starting…"
    );
    assert_eq!(
        pending_approval_label(
            &artisan_frontend::approval_presentation::ApprovalKind::Command,
            true
        ),
        "Denying…"
    );

    let denied = approval_command_for("approval-1", false, "answer-deny");
    let denial = approval_receipt(
        "answer-deny",
        "approval-1",
        false,
        RunInteractionOutcome::Applied,
        ReceiptDisposition::Accepted,
    );
    let pairing = pair_approval_answer(&state, &denied, &denial);
    assert!(pairing.is_settled());

    state.apply(2, &event(approval_resolved("approval-1", false)));
    let settled = state.approval("approval-1").expect("denied row pairs");
    let settled_view = approval_answer_view(settled);
    assert!(!settled_view.is_pending());
    assert_eq!(settled_view.decided(), Some(false));
    match &settled_view {
        ApprovalAnswerView::Decided {
            approved,
            presentation,
        } => {
            assert!(!approved);
            assert_eq!(presentation.title, "Command denied");
            assert_eq!(settled_view.presentation().title, "Command denied");
        }
        ApprovalAnswerView::Pending { .. } => panic!("denied rows render decided"),
    }

    state.apply(3, &event(approval_requested("approval-2")));
    let allowed = approval_command_for("approval-2", true, "answer-allow");
    let approval = approval_receipt(
        "answer-allow",
        "approval-2",
        true,
        RunInteractionOutcome::Applied,
        ReceiptDisposition::Accepted,
    );
    let pairing = pair_approval_answer(&state, &allowed, &approval);
    assert!(pairing.is_settled());
    state.apply(4, &event(approval_resolved("approval-2", true)));

    let order = state
        .approvals_in_order()
        .iter()
        .map(|row| row.approval_id().to_owned())
        .collect::<Vec<String>>();
    assert_eq!(
        order,
        vec![String::from("approval-1"), String::from("approval-2")]
    );
    assert_eq!(
        state.approval("approval-2").expect("row").approved(),
        Some(true)
    );
}

#[test]
fn answer_flight_admits_one_attempt_until_it_settles() {
    let mut flight = AnswerFlight::new();
    assert!(!flight.is_in_flight());
    assert!(flight.begin());
    assert!(flight.is_in_flight());
    assert!(!flight.begin());
    flight.settle();
    assert!(!flight.is_in_flight());
    assert!(flight.begin());
}
