//! Forge-owned composer drafts for [`NativeApplication`].
//!
//! The composer is a view of its scope's Forge draft. Opening a scope reads
//! the draft; every authored change is saved through a per-scope latest-wins
//! chain ([`DraftSync`]) that holds the connection until the Forge has
//! acknowledged the latest save sent; ready images are uploaded to the Forge
//! attachment store and the draft references them. The Forge assigns every
//! revision, so a save always applies and the last to arrive wins. A host switch therefore
//! loses nothing: the new host's view reads its own drafts, and returning to
//! a host shows exactly what its Forge stored.

use std::collections::HashSet;

use artisan_domain::{AuthoredText, SaveComposerDraft, UploadComposerAttachment};

use super::*;
use crate::composer_draft_sync::{DraftBody, DraftSave, DraftSync};
use crate::native_transport_service::{ComposerDraftCommand, ComposerDraftEvent, Hold};

/// Per-connection draft bookkeeping owned by the host view.
#[derive(Default)]
pub(super) struct ComposerDrafts {
    sync: DraftSync<Hold>,
    /// The composer change counter already handed to the sync.
    seen_change: u64,
    /// The scope whose Forge draft was last requested.
    opened: Option<artisan_domain::ComposerDraftScope>,
    /// Composer attachment ids already uploading or uploaded; never re-sent.
    uploads: HashSet<String>,
    /// Draft commands a test-sink application admitted, kept apart from the
    /// sink so they never consume its scripted outcomes.
    #[cfg(test)]
    pub(super) sent: std::cell::RefCell<Vec<ComposerDraftCommand>>,
}

impl NativeApplication {
    /// Follows the composer: reads a newly opened scope's draft, uploads
    /// ready images, and saves authored changes.
    pub(super) fn sync_composer_draft(&mut self, cx: &mut Context<Self>) {
        if self.shutdown_prepared || self.service_stopped {
            return;
        }
        let holds = self.connection_holds();
        let acquire = || holds.as_ref()?.try_hold(HoldKind::Draft);
        let (released, outgoing) = self.composer.update(cx, |composer, _| {
            (
                composer.take_released_draft_scope(),
                composer.take_outgoing_draft(),
            )
        });
        if let Some(released) = released {
            let save = self
                .composer_drafts
                .sync
                .edit(&released, DraftBody::default(), acquire);
            self.submit_draft_save(save);
        }
        // A change typed in the frame of a switch still belongs to the scope
        // it was typed in.
        if let Some(outgoing) = outgoing
            && !outgoing.awaiting
            && outgoing.change != self.composer_drafts.seen_change
        {
            self.composer_drafts.seen_change = outgoing.change;
            let sync = &mut self.composer_drafts.sync;
            let save = sync.edit(&outgoing.scope, outgoing.body, acquire);
            self.submit_draft_save(save);
        }
        let composer = self.composer.read(cx);
        let Some(scope) = composer.draft_scope() else {
            return;
        };
        let awaiting = composer.awaiting_forge_draft();
        let change = composer.draft_change();
        let body = composer.draft_body();
        let unstored = composer.unstored_attachments();
        if awaiting && self.composer_drafts.opened.as_ref() != Some(&scope) {
            self.composer_drafts.opened = Some(scope.clone());
            let _ = self.submit_draft(ComposerDraftCommand::Read(scope.clone()), None);
        }
        for (attachment_id, image) in unstored {
            if !self.composer_drafts.uploads.insert(attachment_id.clone()) {
                continue;
            }
            let Ok(request_id) = create_message_request_id() else {
                continue;
            };
            self.composer_drafts.sync.begin_upload(&scope, acquire);
            let command = ComposerDraftCommand::Upload {
                scope: scope.clone(),
                attachment_id,
                command: Box::new(UploadComposerAttachment { request_id, image }),
            };
            if self
                .submit_draft(command, self.composer_drafts.sync.hold(&scope))
                .is_err()
            {
                self.composer_drafts.sync.finish_upload(&scope);
            }
        }
        // A view still waiting for its Forge draft is not that draft yet.
        if change != self.composer_drafts.seen_change && !awaiting {
            let save = self.composer_drafts.sync.edit(&scope, body, acquire);
            self.submit_draft_save(save);
        }
        self.composer_drafts.seen_change = change;
    }

    /// Applies one draft result from the Forge.
    pub(super) fn receive_draft_event(
        &mut self,
        event: ComposerDraftEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            ComposerDraftEvent::Saved {
                scope, sequence, ..
            }
            | ComposerDraftEvent::SaveFailed {
                scope, sequence, ..
            } => {
                let next = self.composer_drafts.sync.settled(&scope, sequence);
                self.submit_draft_save(next);
            }
            ComposerDraftEvent::Read { scope, result } => {
                let draft = result.ok().flatten();
                let digests = draft.map_or_else(Vec::new, |draft| {
                    self.composer.update(cx, |composer, cx| {
                        composer.apply_forge_draft(&scope, &draft, cx)
                    })
                });
                for digest in digests {
                    let read = ComposerDraftCommand::ReadAttachment {
                        scope: scope.clone(),
                        digest,
                    };
                    let _ = self.submit_draft(read, None);
                }
            }
            ComposerDraftEvent::Uploaded {
                scope,
                attachment_id,
                result,
            } => {
                if let Ok(reference) = result {
                    self.composer.update(cx, |composer, cx| {
                        composer.mark_attachment_stored(&scope, &attachment_id, reference, cx);
                    });
                    // Save the draft that references it now, under the
                    // upload's hold, so a draining connection waits for both.
                    self.sync_composer_draft(cx);
                }
                self.composer_drafts.sync.finish_upload(&scope);
            }
            ComposerDraftEvent::AttachmentRead {
                scope,
                digest,
                result,
            } => {
                if let Ok(stored) = result {
                    self.composer.update(cx, |composer, cx| {
                        composer.restore_stored_attachment(
                            &scope,
                            &digest,
                            stored.mime_type,
                            &stored.bytes,
                            cx,
                        );
                    });
                }
            }
        }
    }

    /// Shows the scope's Forge draft again after the Forge wrote it itself
    /// (an edit recalled a queued message into it). The view waits for the
    /// draft exactly as when the scope opened; a composer that is no longer
    /// empty keeps its text.
    pub(super) fn reload_forge_draft(&mut self, cx: &mut Context<Self>) {
        let scope = self
            .composer
            .update(cx, |composer, _| composer.await_forge_draft());
        if let Some(scope) = scope {
            self.composer_drafts.opened = Some(scope.clone());
            let _ = self.submit_draft(ComposerDraftCommand::Read(scope), None);
        }
    }

    /// Sends every unsent draft body before the connection closes, then
    /// releases the draft holds; each flushed save keeps its own transport
    /// hold, so the close still waits for it.
    pub(super) fn flush_composer_drafts(&mut self, cx: &mut Context<Self>) {
        // Uploads still in flight are referenced by the digest their bytes
        // will be stored under: the save is queued behind them.
        let composer = self.composer.read(cx);
        if let Some(scope) = composer.draft_scope()
            && !composer.awaiting_forge_draft()
        {
            let body = composer.draft_body_with_uploads(&self.composer_drafts.uploads);
            let save = self.composer_drafts.sync.edit(&scope, body, || None);
            self.submit_draft_save(save);
        }
        let saves = self
            .composer_drafts
            .sync
            .flush()
            .into_iter()
            .filter_map(|(save, hold)| Some((draft_save_command(save)?, hold.map(Hold::extend))))
            .collect::<Vec<_>>();
        for (command, hold) in saves {
            let _ = self.submit_draft(command, hold.as_ref());
        }
        self.release_composer_drafts();
    }

    /// Drops every draft hold with the connection that owned it.
    pub(super) fn release_composer_drafts(&mut self) {
        self.composer_drafts.sync.release_all();
        self.composer_drafts.uploads.clear();
        self.composer_drafts.opened = None;
    }

    fn submit_draft_save(&mut self, save: Option<DraftSave>) {
        let mut next = save;
        while let Some(save) = next.take() {
            let scope = save.scope.clone();
            let sequence = save.sequence;
            let admitted = draft_save_command(save).is_some_and(|command| {
                self.submit_draft(command, self.composer_drafts.sync.hold(&scope))
                    .is_ok()
            });
            if !admitted {
                next = self.composer_drafts.sync.settled(&scope, sequence);
            }
        }
    }

    /// Admits one draft command under the scope's hold, so a follow-up save
    /// is admitted even while a host switch drains the connection.
    fn submit_draft(
        &self,
        command: ComposerDraftCommand,
        hold: Option<&Hold>,
    ) -> Result<(), CommandSendError> {
        #[cfg(test)]
        if self.test_command_sink.is_some() {
            self.composer_drafts.sent.borrow_mut().push(command);
            return Ok(());
        }
        let command = NativeTransportCommand::ComposerDraft(command);
        match (hold, self.service.as_ref()) {
            (Some(hold), Some(service)) => service.submit_under(command, hold),
            _ => self.submit_command(command),
        }
    }
}

#[cfg(test)]
impl NativeApplication {
    /// Answers the composer's pending read as the Forge would, with `text`
    /// stored at revision one for its current scope.
    pub(super) fn reply_forge_draft(&mut self, text: &str, cx: &mut Context<Self>) {
        let scope = self
            .composer
            .read(cx)
            .draft_scope()
            .expect("composer scope");
        let draft = artisan_domain::ComposerDraft::new(
            artisan_domain::ComposerDraftRevision::new(1).expect("revision"),
            AuthoredText::parse(text).expect("draft text"),
            Vec::new(),
            artisan_domain::UnixMillis::from_millis(1),
        )
        .expect("draft");
        let result = Ok(Some(draft));
        self.receive_draft_event(ComposerDraftEvent::Read { scope, result }, cx);
    }

    /// Opens `key` in the composer and answers its read with `text`.
    pub(super) fn reopen_with_forge_draft(
        &mut self,
        key: &str,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        self.composer
            .update(cx, |composer, cx| composer.switch_thread(key, false, cx));
        self.reply_forge_draft(text, cx);
    }
}

fn draft_save_command(save: DraftSave) -> Option<ComposerDraftCommand> {
    let request_id = create_message_request_id().ok()?;
    let text = AuthoredText::parse(save.body.text).ok()?;
    let command =
        SaveComposerDraft::new(request_id, save.scope, text, save.body.attachments).ok()?;
    Some(ComposerDraftCommand::Save {
        sequence: save.sequence,
        command: Box::new(command),
    })
}
