//! Sending the composer's Forge draft as a message.
//!
//! The Editor never sends a message body. Send makes sure the Forge has
//! stored the composer's current body, then names the revision it was given
//! (`SubmitComposerDraft`) with the model the composer shows; the Forge
//! admits the send (or refuses it as data), queues exactly that draft once,
//! and empties it. Repeating a send whose answer was lost names the same
//! revision, so the Forge answers the message it already queued instead of
//! queueing another. The request id only correlates the answer.

use artisan_domain::{ComposerDraftRevision, ComposerDraftScope, SubmitComposerDraft};

use super::*;
use crate::composer_draft_sync::SubmitReadiness;

impl NativeApplication {
    /// Starts sending the composer's draft for the flight just launched:
    /// its current body is saved first when the Forge has not stored it.
    pub(super) fn begin_draft_submission(
        &mut self,
        thread_id: &ThreadId,
        body: crate::composer_draft_sync::DraftBody,
        cx: &mut Context<Self>,
    ) {
        let scope = ComposerDraftScope::Thread(thread_id.clone());
        let save = self.composer_drafts.sync.begin_submit(&scope, body);
        self.submit_draft_save(save);
        self.drive_draft_submission(cx);
    }

    /// Sends the waiting flight's draft once the Forge has stored it.
    pub(super) fn drive_draft_submission(&mut self, cx: &mut Context<Self>) {
        let Some(flight) = self.message_flight.as_ref() else {
            return;
        };
        let scope = ComposerDraftScope::Thread(flight.thread_id.clone());
        if !self.composer_drafts.sync.is_submitting(&scope) {
            return;
        }
        let draft_revision = match self.composer_drafts.sync.submit_readiness(&scope) {
            SubmitReadiness::Waiting => return,
            SubmitReadiness::Ready(revision) => revision,
            SubmitReadiness::Failed => {
                self.end_draft_submission(&scope);
                self.fail_waiting_flight(invalid_service_failure(), None, cx);
                return;
            }
        };
        let command = SubmitComposerDraft {
            request_id: flight.request_id.clone(),
            thread_id: flight.thread_id.clone(),
            draft_revision,
            selection: self.displayed_selection(cx),
        };
        let sent = self.submit_command(NativeTransportCommand::SubmitComposerDraft(Box::new(
            command,
        )));
        // Bodies typed after Send are saved behind the send.
        self.end_draft_submission(&scope);
        if let Err(error) = sent {
            self.fail_waiting_flight(command_failure(error), None, cx);
        }
    }

    /// Releases the saves a send held back, for a flight that ended.
    pub(super) fn end_draft_submission(&mut self, scope: &ComposerDraftScope) {
        let next = self.composer_drafts.sync.end_submit(scope);
        self.submit_draft_save(next);
    }

    /// The Forge refused the send because its draft is at another revision
    /// (another writer saved it). The composer keeps its text, which is saved
    /// again; Send then names the revision that save is given.
    pub(super) fn handle_message_stale(
        &mut self,
        thread_id: &ThreadId,
        request_id: &RequestId,
        current_revision: Option<ComposerDraftRevision>,
        cx: &mut Context<Self>,
    ) {
        self.settle_message_flight_hold(Some(request_id));
        let matches = self.message_flight.as_ref().is_some_and(|flight| {
            &flight.thread_id == thread_id && &flight.request_id == request_id
        });
        if !matches {
            return;
        }
        let scope = ComposerDraftScope::Thread(thread_id.clone());
        if let Some(revision) = current_revision {
            self.composer_drafts.sync.observe_revision(&scope, revision);
        }
        self.fail_waiting_flight(
            invalid_service_failure(),
            Some(
                "The draft changed on the Forge before it was sent. Your draft is preserved; press Send again."
                    .to_owned(),
            ),
            cx,
        );
        self.resave_composer_draft(&scope, cx);
    }

    fn fail_waiting_flight(
        &mut self,
        failure: ServiceFailure,
        note: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(flight) = self.message_flight.take() else {
            return;
        };
        self.message_flight_hold = None;
        self.reject_message_submission(flight.token, failure, cx);
        self.message_failure_note = note;
        self.sync_composer_controls(cx);
    }
}
