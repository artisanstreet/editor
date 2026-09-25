//! Answer outcome settlement for [`NativeApplication`].
//!
//! Transport answer outcomes settle exactly the row gate that dispatched the
//! attempt: the event's command is the ground-truth intent, and the mounted
//! surface only accepts the outcome when the gate's latest request identity
//! still names that command. An unmatched or stale outcome is ignored, so a
//! late or duplicate receipt can never corrupt a retried flight. A recorded
//! receipt settles even when no observation rows are retained: retained
//! observations only classify the paired row, they never gate settlement.

use super::{App, Context, Entity, NativeApplication, NativeTransportEvent, ThreadId};
use crate::conversation_surface::ConversationSurface;
use crate::engine_observation_state::EngineObservationState;
use crate::native_transport_service::AnswerFailure;
use artisan_domain::{RespondApproval, RespondQuestion};
use artisan_protocol::{RespondApprovalReceipt, RespondQuestionReceipt};

impl NativeApplication {
    /// Drains one queued answer batch into live transport, once per tick.
    ///
    /// Runs at the head of the controller tick beside the sibling drains, so
    /// a slow or failing transport cannot stall unrelated per-tick work.
    /// Admission follows the established submit path: without it the outbox
    /// is left untouched. Each taken dispatch submits once with its
    /// already-minted request id; `Busy`/`Stopped` keep rows pending with the
    /// existing retry/diagnostic texts, and single-flight holds until
    /// receipts pair through the existing settle-in-place pairing. Draining
    /// first also keeps a same-tick host retirement from dropping gestures.
    pub(super) fn drain_answer_dispatches(&mut self, cx: &mut Context<Self>) {
        if !self.command_submission_is_available() {
            return;
        }
        let Some(host) = self.conversation_host.clone() else {
            return;
        };
        let surface = host.read(cx).surface().clone();
        let this = &*self;
        surface.update(cx, |surface, _| {
            surface.drain_pending_answer_dispatches(&mut |command| this.submit_command(command));
        });
    }
    /// Settles the answer gates for one transport answer outcome.
    ///
    /// Only the four answer variants act; the caller's dispatch arm routes
    /// exactly those here. Answered outcomes pair the recorded receipt
    /// against the exact command that was dispatched. Failed outcomes settle
    /// the gate as failed with the typed transport failure, which reopens it
    /// with the existing retry/diagnostic message rendered in the row.
    pub(super) fn handle_answer_event(
        &mut self,
        event: NativeTransportEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            NativeTransportEvent::ApprovalAnswered { command, receipt } => {
                self.settle_approval_answered(&command, &receipt, cx);
            }
            NativeTransportEvent::ApprovalFailed { command, failure } => {
                self.settle_approval_failed(&command, &failure, cx);
            }
            NativeTransportEvent::QuestionAnswered { command, receipt } => {
                self.settle_question_answered(&command, &receipt, cx);
            }
            NativeTransportEvent::QuestionFailed { command, failure } => {
                self.settle_question_failed(&command, &failure, cx);
            }
            _ => {}
        }
    }

    /// Returns the mounted surface when it owns exactly `thread_id`.
    ///
    /// The mounted host is the only owner of answer gates; an outcome for a
    /// conversation that is not mounted here has no gate to settle.
    fn answer_surface(
        &self,
        thread_id: &ThreadId,
        cx: &App,
    ) -> Option<Entity<ConversationSurface>> {
        let host = self.conversation_host.as_ref()?;
        if &host.read(cx).controller_view().delivery.thread_id != thread_id {
            return None;
        }
        Some(host.read(cx).surface().clone())
    }

    fn settle_approval_answered(
        &mut self,
        command: &RespondApproval,
        receipt: &RespondApprovalReceipt,
        cx: &mut Context<Self>,
    ) {
        let Some(surface) = self.answer_surface(command.thread_id(), cx) else {
            return;
        };
        let empty;
        let state = match self.engine_observations.as_ref() {
            Some(state) if state.thread_id() == command.thread_id() => state,
            _ => {
                empty = EngineObservationState::new(command.thread_id().clone());
                &empty
            }
        };
        surface.update(cx, |surface, surface_cx| {
            surface.settle_approval_answered(state, command, receipt, surface_cx);
        });
    }

    fn settle_approval_failed(
        &mut self,
        command: &RespondApproval,
        failure: &AnswerFailure,
        cx: &mut Context<Self>,
    ) {
        let Some(surface) = self.answer_surface(command.thread_id(), cx) else {
            return;
        };
        let protocol = failure.protocol_failure(command.request_id());
        surface.update(cx, |surface, surface_cx| {
            surface.settle_approval_failure(command, &protocol, surface_cx);
        });
    }

    fn settle_question_answered(
        &mut self,
        command: &RespondQuestion,
        receipt: &RespondQuestionReceipt,
        cx: &mut Context<Self>,
    ) {
        let Some(surface) = self.answer_surface(command.thread_id(), cx) else {
            return;
        };
        let empty;
        let state = match self.engine_observations.as_ref() {
            Some(state) if state.thread_id() == command.thread_id() => state,
            _ => {
                empty = EngineObservationState::new(command.thread_id().clone());
                &empty
            }
        };
        surface.update(cx, |surface, surface_cx| {
            surface.settle_question_answered(state, command, receipt, surface_cx);
        });
    }

    fn settle_question_failed(
        &mut self,
        command: &RespondQuestion,
        failure: &AnswerFailure,
        cx: &mut Context<Self>,
    ) {
        let Some(surface) = self.answer_surface(command.thread_id(), cx) else {
            return;
        };
        let protocol = failure.protocol_failure(command.request_id());
        surface.update(cx, |surface, surface_cx| {
            surface.settle_question_failure(command, &protocol, surface_cx);
        });
    }
}

#[cfg(test)]
#[path = "tests/answer_receipts.rs"]
mod tests;
