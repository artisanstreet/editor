//! Deterministic answer receipt/failure settlement tests against the real
//! application event dispatch and the mounted surface gates.
//!
//! These exercise the same seams as `tests/sends.rs`: a mounted answer
//! surface, the scripted command sink, and direct `handle_service_event`
//! delivery of the transport answer outcomes.

use crate::conversation_host::ConversationHost;
use crate::conversation_surface::ConversationSurface;
use crate::engine_observation_state::EngineObservationState;
use crate::native_application::{NativeApplication, NativeTestCommandSink};
use crate::native_transport_service::{
    AnswerFailure, CommandSendError, NativeTransportCommand, NativeTransportEvent, ServiceFailure,
    ServiceFailureCategory, ServiceFailureStage,
};
use artisan_domain::{ObservationId, ReceiptDisposition, RequestId, RunId, ThreadId};
use artisan_protocol::{RespondApprovalReceipt, RespondQuestionReceipt, RunInteractionOutcome};
use artisan_ui::theme::ThemeMode;
use gpui::{App, Context, Entity, TestAppContext};
use std::{cell::RefCell, collections::VecDeque, rc::Rc};

fn command_sink(
    outcomes: impl IntoIterator<Item = Result<(), CommandSendError>>,
) -> (
    NativeTestCommandSink,
    Rc<RefCell<Vec<NativeTransportCommand>>>,
) {
    let commands = Rc::new(RefCell::new(Vec::new()));
    let sink = NativeTestCommandSink {
        commands: commands.clone(),
        outcomes: Rc::new(RefCell::new(outcomes.into_iter().collect::<VecDeque<_>>())),
    };
    (sink, commands)
}

fn answer_thread() -> ThreadId {
    ThreadId::parse("thread-answer").expect("answer thread")
}

fn answer_run() -> RunId {
    RunId::parse("run-answer").expect("answer run")
}

fn answer_approval() -> ObservationId {
    ObservationId::parse("approval-1").expect("answer approval")
}

fn install_answer_surface(
    application: &mut NativeApplication,
    cx: &mut Context<NativeApplication>,
    sink: NativeTestCommandSink,
) {
    let thread_id = answer_thread();
    let host =
        ConversationHost::mount(thread_id.clone(), ThemeMode::Dark, &mut *cx).expect("answer host");
    application.selected_thread = Some(thread_id);
    application.conversation_host = Some(host);
    application.test_command_sink = Some(sink);
}

fn queue_approval_answer(
    application: &mut NativeApplication,
    cx: &mut Context<NativeApplication>,
) -> RequestId {
    let host = application.conversation_host.clone().expect("answer host");
    let surface = host.read(cx).surface().clone();
    surface.update(cx, |surface, surface_cx| {
        surface.set_answer_context(answer_thread(), answer_run(), surface_cx);
        assert!(surface.submit_approval_gesture(
            "approval-1",
            &answer_approval(),
            true,
            surface_cx,
        ));
        surface.pending_answer_dispatches()[0].request_id.clone()
    })
}

fn recorded_approval(commands: &[NativeTransportCommand]) -> &artisan_domain::RespondApproval {
    assert_eq!(commands.len(), 1);
    if let NativeTransportCommand::RespondApproval(answer) = &commands[0] {
        answer
    } else {
        panic!("tick must submit an approval answer")
    }
}

fn recorded_question(commands: &[NativeTransportCommand]) -> &artisan_domain::RespondQuestion {
    assert_eq!(commands.len(), 1);
    if let NativeTransportCommand::RespondQuestion(answer) = &commands[0] {
        answer
    } else {
        panic!("tick must submit a question answer")
    }
}

fn mounted_surface(application: &NativeApplication, cx: &App) -> Entity<ConversationSurface> {
    application
        .conversation_host
        .clone()
        .expect("answer host")
        .read(cx)
        .surface()
        .clone()
}

fn retryable_failure() -> AnswerFailure {
    ServiceFailure {
        stage: ServiceFailureStage::Request,
        category: ServiceFailureCategory::Backpressure,
    }
    .into()
}

fn approval_receipt(request_id: &RequestId, approved: bool) -> RespondApprovalReceipt {
    RespondApprovalReceipt {
        request_id: request_id.clone(),
        thread_id: answer_thread(),
        run_id: answer_run(),
        approval_id: answer_approval(),
        approved,
        outcome: RunInteractionOutcome::Applied,
        disposition: ReceiptDisposition::Accepted,
    }
}

/// Queues one free-form question gesture through the mounted surface.
fn queue_question_answer(
    application: &mut NativeApplication,
    cx: &mut Context<NativeApplication>,
) -> RequestId {
    let host = application.conversation_host.clone().expect("answer host");
    let surface = host.read(cx).surface().clone();
    surface.update(cx, |surface, surface_cx| {
        surface.set_answer_context(answer_thread(), answer_run(), surface_cx);
        surface.set_question_draft("question-1".to_owned(), "typed answer", surface_cx);
        assert!(surface.submit_question_gesture("question-1", surface_cx));
        surface.pending_answer_dispatches()[0].request_id.clone()
    })
}

#[gpui::test]
fn approval_answered_event_settles_the_surface_gate(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
    let (sink, commands) = command_sink([]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            install_answer_surface(application, cx, sink);
            let request_id = queue_approval_answer(application, cx);
            application.poll_service(cx);
            let command = recorded_approval(&commands.borrow()).clone();
            assert_eq!(command.request_id(), &request_id);
            application.engine_observations = Some(EngineObservationState::new(answer_thread()));

            application.handle_service_event(
                NativeTransportEvent::ApprovalAnswered {
                    command,
                    receipt: approval_receipt(&request_id, true),
                },
                cx,
            );

            let surface = mounted_surface(application, cx);
            let surface = surface.read(cx);
            assert!(
                surface.approval_in_flight("approval-1"),
                "a settled receipt keeps the gate closed until the row resolves"
            );
            assert!(
                surface.approval_failure_message("approval-1").is_none(),
                "a settled receipt clears any earlier failure notice"
            );
        });
    });
}

#[gpui::test]
fn answered_settles_without_retained_observations(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
    let (sink, commands) = command_sink([]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            install_answer_surface(application, cx, sink);
            let request_id = queue_approval_answer(application, cx);
            application.poll_service(cx);
            let command = recorded_approval(&commands.borrow()).clone();
            assert!(application.engine_observations.is_none());

            // A receipt that does not echo the dispatched decision reopens
            // the gate, which is observable proof the settlement ran.
            application.handle_service_event(
                NativeTransportEvent::ApprovalAnswered {
                    command,
                    receipt: approval_receipt(&request_id, false),
                },
                cx,
            );

            let surface = mounted_surface(application, cx);
            let surface = surface.read(cx);
            assert!(
                !surface.approval_in_flight("approval-1"),
                "a recorded receipt settles even without retained observations"
            );
            assert!(surface.approval_failure_message("approval-1").is_some());
        });
    });
}

#[gpui::test]
fn approval_failed_event_reopens_with_the_exact_message(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
    let (sink, commands) = command_sink([]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            install_answer_surface(application, cx, sink);
            queue_approval_answer(application, cx);
            application.poll_service(cx);
            let command = recorded_approval(&commands.borrow()).clone();

            application.handle_service_event(
                NativeTransportEvent::ApprovalFailed {
                    command,
                    failure: retryable_failure(),
                },
                cx,
            );

            let surface = mounted_surface(application, cx);
            let surface = surface.read(cx);
            assert!(
                !surface.approval_in_flight("approval-1"),
                "a failure reopens the retry gate"
            );
            assert_eq!(
                surface.approval_failure_message("approval-1"),
                Some("Could not respond to approval: the answer did not settle; retry the same answer")
            );
        });
    });
}

#[gpui::test]
fn stale_answer_outcomes_cannot_corrupt_a_retried_gate(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
    let (sink, commands) = command_sink([]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            install_answer_surface(application, cx, sink);
            let first_request = queue_approval_answer(application, cx);
            application.poll_service(cx);
            let first_command = recorded_approval(&commands.borrow()).clone();
            application.engine_observations = Some(EngineObservationState::new(answer_thread()));

            let failure = retryable_failure();
            application.handle_service_event(
                NativeTransportEvent::ApprovalFailed {
                    command: first_command.clone(),
                    failure: failure.clone(),
                },
                cx,
            );

            let second_request = queue_approval_answer(application, cx);
            assert_ne!(second_request, first_request);

            let surface = mounted_surface(application, cx);
            {
                let surface = surface.read(cx);
                assert!(
                    surface.approval_in_flight("approval-1"),
                    "the retry re-arms the single-flight gate"
                );
                assert!(surface.approval_failure_message("approval-1").is_none());
            }

            // A duplicate failure and a late receipt for the superseded
            // attempt must both settle nothing.
            application.handle_service_event(
                NativeTransportEvent::ApprovalFailed {
                    command: first_command.clone(),
                    failure,
                },
                cx,
            );
            application.handle_service_event(
                NativeTransportEvent::ApprovalAnswered {
                    command: first_command,
                    receipt: approval_receipt(&first_request, true),
                },
                cx,
            );

            let surface = surface.read(cx);
            assert!(
                surface.approval_in_flight("approval-1"),
                "stale outcomes never reopen or settle the live retry"
            );
            assert!(surface.approval_failure_message("approval-1").is_none());
        });
    });
}

#[gpui::test]
fn duplicate_answered_receipt_is_idempotent(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
    let (sink, commands) = command_sink([]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            install_answer_surface(application, cx, sink);
            let request_id = queue_approval_answer(application, cx);
            application.poll_service(cx);
            let command = recorded_approval(&commands.borrow()).clone();
            application.engine_observations = Some(EngineObservationState::new(answer_thread()));

            for _ in 0..2 {
                application.handle_service_event(
                    NativeTransportEvent::ApprovalAnswered {
                        command: command.clone(),
                        receipt: approval_receipt(&request_id, true),
                    },
                    cx,
                );
            }

            let surface = mounted_surface(application, cx);
            let surface = surface.read(cx);
            assert!(surface.approval_in_flight("approval-1"));
            assert!(surface.approval_failure_message("approval-1").is_none());
        });
    });
}

#[gpui::test]
fn question_answered_event_settles_the_surface_gate(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
    let (sink, commands) = command_sink([]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            install_answer_surface(application, cx, sink);
            let request_id = queue_question_answer(application, cx);
            application.poll_service(cx);
            let command = recorded_question(&commands.borrow()).clone();
            application.engine_observations = Some(EngineObservationState::new(answer_thread()));

            application.handle_service_event(
                NativeTransportEvent::QuestionAnswered {
                    command,
                    receipt: RespondQuestionReceipt {
                        request_id,
                        thread_id: answer_thread(),
                        run_id: answer_run(),
                        question_id: artisan_domain::ObservationId::parse("question-1")
                            .expect("question id"),
                        answers: vec!["typed answer".to_owned()],
                        outcome: RunInteractionOutcome::Applied,
                        disposition: ReceiptDisposition::Accepted,
                    },
                },
                cx,
            );

            let surface = mounted_surface(application, cx);
            let surface = surface.read(cx);
            assert!(surface.question_in_flight("question-1"));
            assert!(surface.question_failure_message("question-1").is_none());
        });
    });
}

#[gpui::test]
fn question_failed_event_reopens_with_the_exact_message(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
    let (sink, commands) = command_sink([]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            install_answer_surface(application, cx, sink);
            queue_question_answer(application, cx);
            application.poll_service(cx);
            let command = recorded_question(&commands.borrow()).clone();

            application.handle_service_event(
                NativeTransportEvent::QuestionFailed {
                    command,
                    failure: retryable_failure(),
                },
                cx,
            );

            let surface = mounted_surface(application, cx);
            let surface = surface.read(cx);
            assert!(
                !surface.question_in_flight("question-1"),
                "a failure reopens the retry gate"
            );
            assert_eq!(
                surface.question_failure_message("question-1"),
                Some("Could not respond to question: the answer did not settle; retry the same answer")
            );
        });
    });
}
