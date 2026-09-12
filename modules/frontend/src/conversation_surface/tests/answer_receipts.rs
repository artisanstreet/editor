//! Surface-level answer settlement: correlation by command identity, the
//! mismatched-receipt diagnostic, and unmatched outcomes settling nothing.

use super::super::{
    AnswerSettlement, ApprovalAnswerGate, ConversationSurface, RespondApprovalAction,
};
use super::{item, scene};
use crate::conversation_scene::SceneItemKind;
use crate::engine_observation_state::EngineObservationState;
use artisan_domain::{Command, ObservationId, ReceiptDisposition, RunId, ThreadId};
use artisan_protocol::{
    ErrorCode, ErrorDetail, ProtocolFailure, RespondApprovalReceipt, RunInteractionOutcome,
};
use artisan_ui::theme::ThemeMode;
use gpui::TestAppContext;

/// Debug selector of the approval failure row for the fixture block.
const APPROVAL_FAILURE_ROW: &str =
    "artisan-conversation-surface-turn-turn_a-block-approval-approval-1-approval-failure";

fn observation_id(value: &str) -> ObservationId {
    ObservationId::parse(value).expect("fixture observation id is valid")
}

fn thread_id() -> ThreadId {
    ThreadId::parse("thread-answer").expect("fixture thread id is valid")
}

fn run_id() -> RunId {
    RunId::parse("run-answer").expect("fixture run id is valid")
}

fn approval_scene() -> crate::conversation_scene::ConversationScene {
    scene(vec![item(
        "approval-1",
        1,
        SceneItemKind::Approval {
            prompt: String::from("Run the test suite?"),
        },
        None,
    )])
}

fn approval_action(approval: &str, approved: bool) -> RespondApprovalAction {
    RespondApprovalAction {
        run_id: run_id(),
        approval_id: observation_id(approval),
        approved,
    }
}

fn command_for(action: &RespondApprovalAction) -> artisan_domain::RespondApproval {
    let mut gate = ApprovalAnswerGate::new();
    let attempt = gate
        .begin(
            thread_id(),
            run_id(),
            action.approval_id.clone(),
            action.approved,
        )
        .expect("fixture gate admits its attempt");
    if let Command::RespondApproval(command) = attempt.command {
        command
    } else {
        panic!("approval gesture must build an approval command")
    }
}

fn dispatched_approval(surface: &mut ConversationSurface) -> artisan_domain::RespondApproval {
    let dispatch = surface
        .take_answer_dispatches()
        .pop()
        .expect("the gesture dispatches");
    if let Command::RespondApproval(command) = dispatch.command {
        command
    } else {
        panic!("approval gesture must build an approval command")
    }
}

fn receipt_for(
    command: &artisan_domain::RespondApproval,
    approved: bool,
) -> RespondApprovalReceipt {
    RespondApprovalReceipt {
        request_id: command.request_id().clone(),
        thread_id: thread_id(),
        run_id: run_id(),
        approval_id: command.approval_id().clone(),
        approved,
        outcome: RunInteractionOutcome::Applied,
        disposition: ReceiptDisposition::Accepted,
    }
}

#[gpui::test]
fn mismatched_approval_receipt_reports_instead_of_settling(cx: &mut TestAppContext) {
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(approval_scene(), ThemeMode::Dark, surface_cx)
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
            let command = dispatched_approval(surface);
            let state = EngineObservationState::new(thread_id());
            // The receipt echoes a decision the command never dispatched.
            let pairing = surface
                .settle_approval_answered(&state, &command, &receipt_for(&command, false), cx)
                .expect("the matching row gate settles the outcome");
            assert!(matches!(
                pairing.settlement,
                AnswerSettlement::Diagnostic { .. }
            ));
            assert!(!surface.approval_in_flight("approval-1"));
            assert!(
                surface
                    .approval_failure_message("approval-1")
                    .expect("the diagnostic is visible in the row")
                    .contains("does not match request")
            );
        });
    });
    cx.run_until_parked();
    assert!(
        cx.debug_bounds(APPROVAL_FAILURE_ROW).is_some(),
        "the typed notice paints through the existing approval failure row"
    );
}

#[gpui::test]
fn unmatched_outcome_settles_nothing(cx: &mut TestAppContext) {
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(approval_scene(), ThemeMode::Dark, surface_cx)
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
            surface.take_answer_dispatches();
            let state = EngineObservationState::new(thread_id());
            let other = command_for(&approval_action("approval-2", true));
            assert!(
                surface
                    .settle_approval_answered(&state, &other, &receipt_for(&other, true), cx)
                    .is_none(),
                "a row with no gate settles nothing"
            );
            let failure = ProtocolFailure {
                code: ErrorCode::Internal,
                detail: ErrorDetail::parse("fixture failure").expect("fixture detail"),
                retryable: true,
                request_id: Some(other.request_id().clone()),
            };
            assert!(
                surface
                    .settle_approval_failure(&other, &failure, cx)
                    .is_none(),
                "a row with no gate settles no failure either"
            );
            assert!(
                surface.approval_in_flight("approval-1"),
                "the unrelated flight is untouched"
            );
            assert!(surface.approval_failure_message("approval-1").is_none());
        });
    });
}
