//! Engine answer submit wiring: explicit gestures, fresh identities,
//! single-flight suppression, client-side empty rejection, and
//! settle-in-place resolution through the existing pairing policy.
//!
//! These are plain `#[test]` functions against the surface's submit gates
//! with no display dependencies.

use artisan_domain::{
    ApprovalObservation, ApprovalRequest, Command, EngineObservationEvent,
    OBSERVATION_ANSWER_MAX_BYTES, Observation, ObservationId, ObservationSequence, QuestionInput,
    QuestionObservation, QuestionOption, ReceiptDisposition, RunId, ThreadId,
};
use artisan_protocol::{
    ErrorCode, ErrorDetail, ProtocolFailure, RespondApprovalReceipt, RespondQuestionReceipt,
    RunInteractionOutcome,
};

use super::super::{
    AnswerDispatch, AnswerDispatchAction, ApprovalAnswerGate, ConversationSurface,
    QuestionAnswerGate, QuestionChoiceCache, drain_answer_queue,
};
use super::{item, scene};
use crate::conversation_scene::SceneItemKind;
use crate::engine_approve_ui::AnswerSettlement;
use crate::engine_observation_state::EngineObservationState;
use crate::native_transport_service::{CommandSendError, NativeTransportCommand};
use artisan_ui::theme::ThemeMode;
use gpui::{Entity, KeyDownEvent, Keystroke, TestAppContext, VisualTestContext};

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

fn event(observation: Observation) -> EngineObservationEvent {
    EngineObservationEvent {
        thread_id: thread_id(),
        observation,
        // Integrated domain carries the optional engine attribution
        // row on every observation event; fixtures leave it absent.
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

fn question_input(question: &str, multi_select: bool) -> QuestionInput {
    QuestionInput {
        question_id: observation_id(question),
        text: String::from("Which runtime?"),
        header: Some(String::from("Runtime")),
        multi_select,
        options: Some(first_options()),
    }
}

fn question_requested(question: &str, multi_select: bool) -> Observation {
    Observation::Question(
        QuestionObservation::requested(
            observation_id(&format!("obs-{question}-requested")),
            sequence(11),
            question_input(question, multi_select),
        )
        .expect("fixture requested question is valid"),
    )
}

fn approval_command_inner(command: &Command) -> &artisan_domain::RespondApproval {
    match command {
        Command::RespondApproval(command) => command,
        _ => panic!("approval gesture must build an approval command"),
    }
}

fn question_command_inner(command: &Command) -> &artisan_domain::RespondQuestion {
    match command {
        Command::RespondQuestion(command) => command,
        _ => panic!("question gesture must build a question command"),
    }
}

fn approval_dispatch() -> AnswerDispatch {
    let mut gate = ApprovalAnswerGate::new();
    let attempt = gate
        .begin(thread_id(), run_id(), observation_id("approval-1"), true)
        .expect("approve gesture admits an attempt");
    AnswerDispatch {
        action: AnswerDispatchAction::Approval(attempt.action),
        command: attempt.command,
        request_id: attempt.request_id,
    }
}

fn question_dispatch() -> AnswerDispatch {
    let mut gate = QuestionAnswerGate::new();
    let attempt = gate
        .submit_single(
            thread_id(),
            run_id(),
            observation_id("question-1"),
            String::from("tokio"),
        )
        .expect("single-select choice submits immediately");
    AnswerDispatch {
        action: AnswerDispatchAction::Question(attempt.action),
        command: attempt.command,
        request_id: attempt.request_id,
    }
}

#[test]
fn approve_submit_dispatches_with_fresh_ids() {
    let mut first_row = ApprovalAnswerGate::new();
    let mut second_row = ApprovalAnswerGate::new();
    let first = first_row
        .begin(thread_id(), run_id(), observation_id("approval-1"), true)
        .expect("approve gesture admits an attempt");
    let second = second_row
        .begin(thread_id(), run_id(), observation_id("approval-2"), true)
        .expect("second row admits its own attempt");
    assert_ne!(first.request_id, second.request_id);
    assert!(first.action.approved);
    assert_eq!(first.action.approval_id.as_str(), "approval-1");
    assert_eq!(first.action.run_id, run_id());
    let command = approval_command_inner(&first.command);
    assert_eq!(command.request_id(), &first.request_id);
    assert_eq!(command.thread_id(), &thread_id());
    assert!(command.approved());
    assert!(first_row.is_in_flight());
    assert_eq!(first_row.pending_decision(), Some(true));
}

#[test]
fn deny_submit_carries_an_explicit_denial() {
    let mut gate = ApprovalAnswerGate::new();
    let attempt = gate
        .begin(thread_id(), run_id(), observation_id("approval-1"), false)
        .expect("deny gesture admits an attempt");
    assert!(!attempt.action.approved);
    assert!(!approval_command_inner(&attempt.command).approved());
    assert_eq!(gate.pending_decision(), Some(false));
}

#[test]
fn question_choice_submit_carries_selected_options() {
    let mut gate = QuestionAnswerGate::new();
    assert!(
        QuestionChoiceCache {
            multi_select: true,
            options: vec![
                (String::from("tokio"), None),
                (
                    String::from("async-std"),
                    Some(String::from("alternative runtime")),
                ),
            ],
        }
        .is_choice()
    );
    gate.toggle_option(String::from("tokio"), true);
    gate.toggle_option(String::from("async-std"), true);
    assert_eq!(
        gate.selected(),
        &[String::from("tokio"), String::from("async-std")]
    );
    let attempt = gate
        .submit_selected(thread_id(), run_id(), observation_id("question-1"))
        .expect("staged choices submit");
    assert_eq!(
        attempt.action.answers,
        vec![String::from("tokio"), String::from("async-std")]
    );
    assert_eq!(
        question_command_inner(&attempt.command).answers(),
        &attempt.action.answers
    );
    assert!(gate.is_in_flight());
}

#[test]
fn freeform_submit_carries_typed_text() {
    let mut gate = QuestionAnswerGate::new();
    gate.set_draft("  typed answer  ");
    let attempt = gate
        .submit_freeform(thread_id(), run_id(), observation_id("question-free"))
        .expect("typed draft submits");
    assert_eq!(attempt.action.answers, vec![String::from("typed answer")]);
    assert_eq!(gate.draft(), "");
    assert!(gate.is_in_flight());
}

#[test]
fn empty_freeform_rejected_without_dispatch() {
    let mut gate = QuestionAnswerGate::new();
    gate.set_draft("   ");
    assert!(
        gate.submit_freeform(thread_id(), run_id(), observation_id("question-free"))
            .is_none()
    );
    assert!(!gate.is_in_flight());
    assert!(gate.last_request_id().is_none());
    assert!(gate.failure_message().is_none());
    gate.set_draft("typed");
    assert!(
        gate.submit_freeform(thread_id(), run_id(), observation_id("question-free"))
            .is_some(),
        "the row stays pending so a later typed submit still works"
    );
}

#[test]
fn double_submit_suppressed_while_flight_outstanding() {
    let mut gate = ApprovalAnswerGate::new();
    let first = gate
        .begin(thread_id(), run_id(), observation_id("approval-1"), true)
        .expect("first gesture admits an attempt");
    assert!(
        gate.begin(thread_id(), run_id(), observation_id("approval-1"), true,)
            .is_none(),
        "a second gesture mints nothing while the first is outstanding"
    );
    assert_eq!(gate.last_request_id(), Some(&first.request_id));
    let failure = ProtocolFailure {
        code: ErrorCode::Internal,
        detail: ErrorDetail::parse("fixture unavailable").expect("fixture detail is valid"),
        retryable: true,
        request_id: Some(first.request_id.clone()),
    };
    let settlement = gate.settle_failure(&first.request_id, &failure);
    match &settlement {
        AnswerSettlement::RetryableFailure { message } => {
            assert!(message.contains("retry the same answer"));
        }
        _ => panic!("unavailable work must stay retryable"),
    }
    assert!(!gate.is_in_flight());
    let retry = gate
        .begin(thread_id(), run_id(), observation_id("approval-1"), true)
        .expect("an explicit retry mints a fresh identity");
    assert_ne!(retry.request_id, first.request_id);
}

#[test]
fn resolution_settles_the_row_via_existing_pairing() {
    let mut state = EngineObservationState::new(thread_id());
    let _ = state.apply(1, &event(approval_requested("approval-1")));
    let _ = state.apply(2, &event(question_requested("question-1", false)));

    let mut gate = ApprovalAnswerGate::new();
    let attempt = gate
        .begin(thread_id(), run_id(), observation_id("approval-1"), true)
        .expect("approve gesture admits an attempt");
    let receipt = RespondApprovalReceipt {
        request_id: attempt.request_id.clone(),
        thread_id: thread_id(),
        run_id: run_id(),
        approval_id: observation_id("approval-1"),
        approved: true,
        outcome: RunInteractionOutcome::Applied,
        disposition: ReceiptDisposition::Accepted,
    };
    let pairing = gate.settle_receipt(&state, approval_command_inner(&attempt.command), &receipt);
    assert_eq!(
        pairing.settlement,
        AnswerSettlement::SettledInPlace { duplicate: false }
    );
    assert!(pairing.is_settled());

    let _ = state.apply(3, &event(approval_resolved("approval-1", true)));
    assert_eq!(
        state.approval("approval-1").expect("row pairs").approved(),
        Some(true)
    );
    assert_eq!(state.approvals_in_order().len(), 1);

    let mut question_gate = QuestionAnswerGate::new();
    let question_attempt = question_gate
        .submit_single(
            thread_id(),
            run_id(),
            observation_id("question-1"),
            String::from("tokio"),
        )
        .expect("single-select choice submits immediately");
    let question_receipt = RespondQuestionReceipt {
        request_id: question_attempt.request_id.clone(),
        thread_id: thread_id(),
        run_id: run_id(),
        question_id: observation_id("question-1"),
        answers: vec![String::from("tokio")],
        outcome: RunInteractionOutcome::Applied,
        disposition: ReceiptDisposition::Accepted,
    };
    let question_pairing = question_gate.settle_receipt(
        &state,
        question_command_inner(&question_attempt.command),
        &question_receipt,
    );
    assert!(question_pairing.is_settled());
}

/// Scripted transport submit harness mirroring the application
/// `test_command_sink`: records every submitted transport command and
/// replays scripted admission outcomes in order, defaulting to
/// admitted once the script runs out.
struct ScriptedSubmit {
    commands: Vec<NativeTransportCommand>,
    outcomes: std::collections::VecDeque<Result<(), CommandSendError>>,
}

impl ScriptedSubmit {
    fn new(outcomes: Vec<Result<(), CommandSendError>>) -> Self {
        Self {
            commands: Vec::new(),
            outcomes: outcomes.into(),
        }
    }

    fn submit(&mut self, command: NativeTransportCommand) -> Result<(), CommandSendError> {
        self.commands.push(command);
        self.outcomes.pop_front().unwrap_or(Ok(()))
    }
}

fn approval_answer(command: &NativeTransportCommand) -> &artisan_domain::RespondApproval {
    if let NativeTransportCommand::RespondApproval(answer) = command {
        answer
    } else {
        panic!("dispatch must submit an approval answer")
    }
}

fn question_answer(command: &NativeTransportCommand) -> &artisan_domain::RespondQuestion {
    if let NativeTransportCommand::RespondQuestion(answer) = command {
        answer
    } else {
        panic!("dispatch must submit a question answer")
    }
}

#[test]
fn drain_sends_taken_queue_preserving_minted_ids() {
    let approval = approval_dispatch();
    let question = question_dispatch();
    let expected_approval = approval.request_id.clone();
    let expected_question = question.request_id.clone();
    let mut submit = ScriptedSubmit::new(Vec::new());
    let (requeue, report) = drain_answer_queue(vec![approval, question], &mut |command| {
        submit.submit(command)
    });
    assert_eq!(report.sent, 2);
    assert!(report.is_clean());
    assert!(requeue.is_empty());
    assert_eq!(submit.commands.len(), 2);
    let submitted = approval_answer(&submit.commands[0]);
    assert_eq!(submitted.request_id(), &expected_approval);
    assert_eq!(submitted.thread_id(), &thread_id());
    assert_eq!(submitted.run_id(), &run_id());
    assert_eq!(submitted.approval_id().as_str(), "approval-1");
    assert!(submitted.approved);
    let submitted = question_answer(&submit.commands[1]);
    assert_eq!(submitted.request_id(), &expected_question);
    assert_eq!(submitted.thread_id(), &thread_id());
    assert_eq!(submitted.run_id(), &run_id());
    assert_eq!(submitted.question_id().as_str(), "question-1");
    assert_eq!(submitted.answers(), &vec![String::from("tokio")]);
}

#[test]
fn drain_failure_requeues_with_retry_message() {
    let dispatch = approval_dispatch();
    let expected = dispatch.request_id.clone();
    let mut submit = ScriptedSubmit::new(vec![Err(CommandSendError::Busy)]);
    let (requeue, report) =
        drain_answer_queue(vec![dispatch], &mut |command| submit.submit(command));
    assert_eq!(submit.commands.len(), 1, "one drain attempt per dispatch");
    assert_eq!(report.sent, 0);
    assert!(!report.is_clean());
    assert_eq!(report.failed.len(), 1);
    assert_eq!(report.failed[0].request_id, expected);
    assert!(
        report.failed[0].message.contains("retry the same answer"),
        "the existing retry message is surfaced"
    );
    assert_eq!(requeue.len(), 1);
    assert_eq!(requeue[0].request_id, expected);

    let mut submit = ScriptedSubmit::new(Vec::new());
    let (requeue, report) = drain_answer_queue(requeue, &mut |command| submit.submit(command));
    assert_eq!(report.sent, 1);
    assert!(report.is_clean());
    assert!(requeue.is_empty());
    assert_eq!(submit.commands.len(), 1);
    assert_eq!(approval_answer(&submit.commands[0]).request_id, expected);
}

#[test]
fn drain_stopped_reports_a_diagnostic() {
    let dispatch = question_dispatch();
    let expected = dispatch.request_id.clone();
    let mut submit = ScriptedSubmit::new(vec![Err(CommandSendError::Stopped)]);
    let (requeue, report) =
        drain_answer_queue(vec![dispatch], &mut |command| submit.submit(command));
    assert_eq!(report.sent, 0);
    assert_eq!(report.failed.len(), 1);
    assert_eq!(report.failed[0].request_id, expected);
    assert!(
        report.failed[0].message.contains("nothing was recorded"),
        "a stopped service degrades to the existing diagnostic text"
    );
    assert_eq!(requeue.len(), 1, "nothing is silently dropped");
}

#[test]
fn drain_empty_outbox_is_a_no_op() {
    let mut submit = ScriptedSubmit::new(vec![Err(CommandSendError::Busy)]);
    let (requeue, report) = drain_answer_queue(Vec::new(), &mut |command| submit.submit(command));
    assert_eq!(report.sent, 0);
    assert!(report.is_clean());
    assert!(requeue.is_empty());
    assert!(
        submit.commands.is_empty(),
        "an empty queue never touches submit"
    );
}

#[test]
fn double_drain_sends_once() {
    let dispatch = approval_dispatch();
    let expected = dispatch.request_id.clone();
    let mut submit = ScriptedSubmit::new(Vec::new());
    let (requeue, first) =
        drain_answer_queue(vec![dispatch], &mut |command| submit.submit(command));
    let (requeue, second) = drain_answer_queue(requeue, &mut |command| submit.submit(command));
    assert_eq!((first.sent, second.sent), (1, 0));
    assert!(second.is_clean());
    assert!(requeue.is_empty());
    assert_eq!(submit.commands.len(), 1);
    assert_eq!(approval_answer(&submit.commands[0]).request_id, expected);
}

#[gpui::test]
fn surface_drain_takes_sends_and_empties(cx: &mut TestAppContext) {
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(scene(Vec::new()), ThemeMode::Dark, surface_cx)
    });
    cx.update(|_, app| {
        surface.update(app, |surface, cx| {
            surface.set_answer_context(thread_id(), run_id(), cx);
            assert!(surface.submit_approval_gesture(
                "approval-1",
                &observation_id("approval-1"),
                true,
                cx,
            ));
            assert_eq!(surface.pending_answer_dispatches().len(), 1);
        });
    });
    let mut submit = ScriptedSubmit::new(Vec::new());
    cx.update(|_, app| {
        surface.update(app, |surface, _| {
            let report =
                surface.drain_pending_answer_dispatches(&mut |command| submit.submit(command));
            assert_eq!(report.sent, 1);
            assert!(report.is_clean());
            assert!(surface.pending_answer_dispatches().is_empty());
            let replay =
                surface.drain_pending_answer_dispatches(&mut |command| submit.submit(command));
            assert_eq!(replay.sent, 0);
            assert!(surface.pending_answer_dispatches().is_empty());
        });
    });
    assert_eq!(submit.commands.len(), 1);
    let submitted = approval_answer(&submit.commands[0]);
    assert_eq!(submitted.thread_id(), &thread_id());
    assert_eq!(submitted.run_id(), &run_id());
    assert_eq!(submitted.approval_id().as_str(), "approval-1");
    assert!(submitted.approved);
}

#[test]
fn keystroke_text_accumulates_with_canonical_intake() {
    let mut gate = QuestionAnswerGate::new();
    assert!(gate.insert_text("h"));
    assert!(gate.insert_text("i"));
    assert_eq!(gate.draft(), "hi");
    assert!(gate.insert_text("a\r\nb"));
    assert_eq!(gate.draft(), "hiab");
    assert!(gate.insert_text("x\u{200B}y"));
    assert_eq!(gate.draft(), "hiabxy");
    assert!(!gate.insert_text(""));
}

#[test]
fn keystroke_intake_clamped_to_answer_bound() {
    let mut gate = QuestionAnswerGate::new();
    assert!(gate.insert_text(&"y".repeat(OBSERVATION_ANSWER_MAX_BYTES)));
    assert_eq!(gate.draft().len(), OBSERVATION_ANSWER_MAX_BYTES);
    assert!(!gate.insert_text("z"));
    assert_eq!(gate.draft().len(), OBSERVATION_ANSWER_MAX_BYTES);
}

#[test]
fn delete_backward_removes_last_char() {
    let mut gate = QuestionAnswerGate::new();
    assert!(!gate.delete_backward());
    gate.set_draft("hi");
    assert!(gate.delete_backward());
    assert_eq!(gate.draft(), "h");
    assert!(gate.delete_backward());
    assert_eq!(gate.draft(), "");
    assert!(!gate.delete_backward());
}

#[test]
fn settled_row_drops_its_draft() {
    let mut state = EngineObservationState::new(thread_id());
    let _ = state.apply(1, &event(question_requested("question-1", false)));
    let mut gate = QuestionAnswerGate::new();
    gate.set_draft("typed");
    let attempt = gate
        .submit_freeform(thread_id(), run_id(), observation_id("question-1"))
        .expect("typed draft submits");
    gate.set_draft("typed during flight");
    let receipt = RespondQuestionReceipt {
        request_id: attempt.request_id.clone(),
        thread_id: thread_id(),
        run_id: run_id(),
        question_id: observation_id("question-1"),
        answers: vec![String::from("typed")],
        outcome: RunInteractionOutcome::Applied,
        disposition: ReceiptDisposition::Accepted,
    };
    let pairing = gate.settle_receipt(&state, question_command_inner(&attempt.command), &receipt);
    assert!(pairing.is_settled());
    assert_eq!(gate.draft(), "");
}

#[test]
fn failed_row_keeps_its_draft_for_retry() {
    let mut gate = QuestionAnswerGate::new();
    gate.set_draft("typed");
    let attempt = gate
        .submit_freeform(thread_id(), run_id(), observation_id("question-1"))
        .expect("typed draft submits");
    gate.set_draft("retry text");
    let failure = ProtocolFailure {
        code: ErrorCode::Internal,
        detail: ErrorDetail::parse("fixture unavailable").expect("fixture detail is valid"),
        retryable: true,
        request_id: Some(attempt.request_id.clone()),
    };
    let settlement = gate.settle_failure(&attempt.request_id, &failure);
    assert!(matches!(
        settlement,
        AnswerSettlement::RetryableFailure { .. }
    ));
    assert_eq!(gate.draft(), "retry text");
    assert!(!gate.is_in_flight());
}

fn freeform_scene() -> crate::conversation_scene::ConversationScene {
    scene(vec![
        item(
            "question-a",
            1,
            SceneItemKind::Question {
                prompt: String::from("Which runtime?"),
            },
            None,
        ),
        item(
            "question-b",
            2,
            SceneItemKind::Question {
                prompt: String::from("Which region?"),
            },
            None,
        ),
    ])
}

fn press_key(cx: &mut VisualTestContext, key: &str) {
    cx.simulate_event(KeyDownEvent {
        keystroke: Keystroke::parse(key).expect("known test key"),
        is_held: false,
        prefer_character_input: false,
    });
}

fn focus_row(cx: &mut VisualTestContext, surface: &Entity<ConversationSurface>, block: &str) {
    cx.update(|window, app| {
        window.focus(
            &surface
                .read(app)
                .question_focus_handle(block)
                .expect("row focus handle"),
            app,
        );
    });
    cx.run_until_parked();
}

fn row_draft(
    cx: &mut VisualTestContext,
    surface: &Entity<ConversationSurface>,
    block: &str,
) -> String {
    cx.update(|_, app| {
        surface
            .read(app)
            .question_gates
            .get(block)
            .map_or("", QuestionAnswerGate::draft)
            .to_owned()
    })
}

#[gpui::test]
fn freeform_typing_accumulates_per_row(cx: &mut TestAppContext) {
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(freeform_scene(), ThemeMode::Dark, surface_cx)
    });
    cx.run_until_parked();
    focus_row(cx, &surface, "question-a");
    press_key(cx, "h");
    press_key(cx, "i");
    press_key(cx, "space");
    press_key(cx, "h");
    press_key(cx, "o");
    cx.run_until_parked();
    assert_eq!(row_draft(cx, &surface, "question-a"), "hi ho");
    focus_row(cx, &surface, "question-b");
    press_key(cx, "x");
    cx.run_until_parked();
    assert_eq!(row_draft(cx, &surface, "question-b"), "x");
    assert_eq!(
        row_draft(cx, &surface, "question-a"),
        "hi ho",
        "rows keep isolated drafts"
    );
}

#[gpui::test]
fn freeform_backspace_deletes(cx: &mut TestAppContext) {
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(freeform_scene(), ThemeMode::Dark, surface_cx)
    });
    cx.run_until_parked();
    focus_row(cx, &surface, "question-a");
    press_key(cx, "h");
    press_key(cx, "i");
    press_key(cx, "backspace");
    cx.run_until_parked();
    assert_eq!(row_draft(cx, &surface, "question-a"), "h");
}

#[gpui::test]
fn freeform_enter_submits_staged_text(cx: &mut TestAppContext) {
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(freeform_scene(), ThemeMode::Dark, surface_cx)
    });
    cx.run_until_parked();
    cx.update(|_, app| {
        surface.update(app, |surface, cx| {
            surface.set_answer_context(thread_id(), run_id(), cx);
        });
    });
    focus_row(cx, &surface, "question-a");
    for key in ["t", "o", "k", "i", "o"] {
        press_key(cx, key);
    }
    press_key(cx, "enter");
    cx.run_until_parked();
    let answers = cx.update(|_, app| {
        let surface = surface.read(app);
        assert_eq!(surface.pending_answer_dispatches().len(), 1);
        match &surface.pending_answer_dispatches()[0].command {
            Command::RespondQuestion(command) => command.answers().clone(),
            _ => panic!("free-form enter must dispatch an answer"),
        }
    });
    assert_eq!(answers, vec![String::from("tokio")]);
    assert_eq!(row_draft(cx, &surface, "question-a"), "");
}

#[gpui::test]
fn freeform_empty_enter_rejected(cx: &mut TestAppContext) {
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(freeform_scene(), ThemeMode::Dark, surface_cx)
    });
    cx.run_until_parked();
    cx.update(|_, app| {
        surface.update(app, |surface, cx| {
            surface.set_answer_context(thread_id(), run_id(), cx);
        });
    });
    focus_row(cx, &surface, "question-b");
    press_key(cx, "enter");
    cx.run_until_parked();
    cx.update(|_, app| {
        assert!(surface.read(app).pending_answer_dispatches().is_empty());
    });
    assert_eq!(row_draft(cx, &surface, "question-b"), "");
}

#[gpui::test]
fn freeform_escape_clears_focus(cx: &mut TestAppContext) {
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(freeform_scene(), ThemeMode::Dark, surface_cx)
    });
    cx.run_until_parked();
    focus_row(cx, &surface, "question-a");
    press_key(cx, "x");
    press_key(cx, "escape");
    cx.run_until_parked();
    cx.update(|window, app| {
        let view = surface.read(app);
        assert!(
            !view
                .question_focus_handle("question-a")
                .expect("row focus handle")
                .is_focused(window),
            "escape must clear row focus without submitting"
        );
        assert!(
            view.transcript_focus_handle().is_focused(window),
            "escape returns focus to the transcript"
        );
    });
    assert_eq!(row_draft(cx, &surface, "question-a"), "x");
    cx.update(|_, app| {
        assert!(surface.read(app).pending_answer_dispatches().is_empty());
    });
}
