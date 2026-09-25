//! The composer's side of its Forge-owned draft.
//!
//! The composer is a view: it reports each authored change through
//! [`NativeComposer::draft_change`] and [`NativeComposer::draft_body`], and
//! shows the draft the Forge returns for a newly opened scope. Undo and redo
//! stay local editing history of the current view.

#![forbid(unsafe_code)]

use artisan_domain::{
    ComposerAttachmentDigest, ComposerAttachmentRef, ComposerDraft, ComposerDraftScope,
    ComposerImage, ImageMimeType, ProjectId, ThreadId,
};

use sha2::{Digest as _, Sha256};

use super::*;
use crate::composer_draft_sync::DraftBody;

/// The draft scope a composer key names: `project:<id>` for a project's
/// new-task composer, otherwise a thread id.
fn scope_for_key(key: &str) -> Option<ComposerDraftScope> {
    match key.strip_prefix("project:") {
        Some(project) => ProjectId::parse(project)
            .ok()
            .map(ComposerDraftScope::Project),
        None => ThreadId::parse(key).ok().map(ComposerDraftScope::Thread),
    }
}

/// The draft a switch left behind, captured before the view rebinds so a
/// change typed in the same frame still reaches its own scope.
pub(crate) struct OutgoingDraft {
    /// Scope the view showed.
    pub(crate) scope: ComposerDraftScope,
    /// Its content at the switch.
    pub(crate) body: DraftBody,
    /// The change counter at the switch.
    pub(crate) change: u64,
    /// Whether the view was still waiting for that scope's Forge draft.
    pub(crate) awaiting: bool,
}

impl NativeComposer {
    pub(super) fn capture_draft(&self, key: &str) -> Option<OutgoingDraft> {
        Some(OutgoingDraft {
            scope: scope_for_key(key)?,
            body: self.draft_body(),
            change: self.draft_change,
            awaiting: self.awaiting_forge_draft,
        })
    }

    /// The draft the last ordinary switch left behind, once.
    pub(crate) fn take_outgoing_draft(&mut self) -> Option<OutgoingDraft> {
        self.outgoing_draft.take()
    }

    /// Records an authored change the Forge draft must receive. Local edits
    /// win over a Forge draft that has not arrived yet.
    pub(super) fn note_draft_change(&mut self) {
        self.draft_change = self.draft_change.wrapping_add(1);
        self.awaiting_forge_draft = false;
    }

    /// Advances on every authored change of the current draft.
    pub(crate) const fn draft_change(&self) -> u64 {
        self.draft_change
    }

    /// The Forge draft scope this composer currently shows.
    pub(crate) fn draft_scope(&self) -> Option<ComposerDraftScope> {
        scope_for_key(self.draft_thread.as_deref()?)
    }

    /// Whether the composer is waiting to show its scope's Forge draft.
    pub(crate) const fn awaiting_forge_draft(&self) -> bool {
        self.awaiting_forge_draft
    }

    /// The scope a carried draft moved away from, once.
    pub(crate) fn take_released_draft_scope(&mut self) -> Option<ComposerDraftScope> {
        scope_for_key(&self.released_scope.take()?)
    }

    /// The content the Forge draft stores: the text exactly as typed and
    /// every stored attachment in tray order. Images still preparing or
    /// uploading join once stored.
    pub(crate) fn draft_body(&self) -> DraftBody {
        DraftBody {
            text: self.state.draft().to_owned(),
            attachments: self
                .attachments
                .iter()
                .filter_map(|attachment| attachment.stored.clone())
                .collect(),
        }
    }

    /// The draft body with every uploading attachment referenced by the
    /// reference its upload will store (the digest of its bytes), for a
    /// save queued behind those uploads as the connection closes.
    pub(crate) fn draft_body_with_uploads(&self, uploading: &HashSet<String>) -> DraftBody {
        let attachments = self
            .attachments
            .iter()
            .filter_map(|attachment| {
                if let Some(stored) = &attachment.stored {
                    return Some(stored.clone());
                }
                let bytes = attachment
                    .bytes
                    .as_ref()
                    .filter(|_| uploading.contains(&attachment.id))?;
                let digest = ComposerAttachmentDigest::new(Sha256::digest(bytes.as_slice()).into());
                let mime_type = ImageMimeType::parse(&attachment.mime_type).ok()?;
                let size = u32::try_from(bytes.len()).ok()?;
                ComposerAttachmentRef::new(digest, mime_type, &attachment.name, size).ok()
            })
            .collect();
        DraftBody {
            text: self.state.draft().to_owned(),
            attachments,
        }
    }

    /// Ready attachments the Forge store does not hold yet, by composer id.
    pub(crate) fn unstored_attachments(&self) -> Vec<(String, ComposerImage)> {
        self.attachments
            .iter()
            .filter(|attachment| attachment.stored.is_none())
            .filter_map(|attachment| {
                let bytes = attachment.bytes.as_ref()?;
                let image = ComposerImage::new(
                    &attachment.mime_type,
                    bytes.as_ref().clone(),
                    &attachment.name,
                )
                .ok()?;
                Some((attachment.id.clone(), image))
            })
            .collect()
    }

    /// Records the Forge store's reference for an uploaded attachment that is
    /// still in this scope's tray.
    pub(crate) fn mark_attachment_stored(
        &mut self,
        scope: &ComposerDraftScope,
        attachment_id: &str,
        reference: ComposerAttachmentRef,
        cx: &mut Context<Self>,
    ) {
        if self.draft_scope().as_ref() != Some(scope) {
            return;
        }
        let Some(attachment) = self
            .attachments
            .iter_mut()
            .find(|attachment| attachment.id == attachment_id && attachment.bytes.is_some())
        else {
            return;
        };
        attachment.stored = Some(reference);
        self.note_draft_change();
        cx.notify();
    }

    /// Waits for the current scope's Forge draft again, when the Forge wrote
    /// it (a recalled queued message). Only an empty composer waits; its
    /// scope is returned for the read.
    pub(crate) fn await_forge_draft(&mut self) -> Option<ComposerDraftScope> {
        if !self.state.draft().is_empty() || !self.attachments.is_empty() {
            return None;
        }
        self.awaiting_forge_draft = true;
        self.draft_scope()
    }

    /// Shows the Forge draft of the scope just opened, unless the user has
    /// already typed into it. Returns the digests whose bytes must be read.
    pub(crate) fn apply_forge_draft(
        &mut self,
        scope: &ComposerDraftScope,
        draft: &ComposerDraft,
        cx: &mut Context<Self>,
    ) -> Vec<ComposerAttachmentDigest> {
        if !self.awaiting_forge_draft || self.draft_scope().as_ref() != Some(scope) {
            return Vec::new();
        }
        self.awaiting_forge_draft = false;
        self.replace_draft_text(draft.text().as_str().to_owned());
        let mut reserved = HashSet::new();
        for reference in draft.attachments() {
            let Some(id) = self.next_unique_attachment_id(&mut reserved) else {
                break;
            };
            let mime_type = reference.mime_type().as_str();
            let mut attachment = ComposerAttachment::pending(
                id,
                reference.name(),
                ImageFormat::from_mime_type(mime_type),
                mime_type,
                usize::try_from(reference.size_bytes()).unwrap_or(usize::MAX),
            );
            attachment.stored = Some(reference.clone());
            self.attachments.push(attachment);
        }
        cx.notify();
        let mut digests = draft
            .attachments()
            .iter()
            .map(|reference| *reference.digest())
            .collect::<Vec<_>>();
        digests.dedup();
        digests
    }

    /// Types `text` at the end of the draft, as keystrokes would.
    #[cfg(test)]
    pub(crate) fn type_at_end(&mut self, text: &str, cx: &mut Context<Self>) {
        let end = self.state.draft().len();
        self.replace_range(end..end, text, None, cx);
    }

    /// Prepares the thumbnails of restored attachments once their stored
    /// bytes arrive. The bytes are kept exactly as stored.
    pub(crate) fn restore_stored_attachment(
        &mut self,
        scope: &ComposerDraftScope,
        digest: &ComposerAttachmentDigest,
        mime_type: ImageMimeType,
        bytes: &[u8],
        cx: &mut Context<Self>,
    ) {
        if self.draft_scope().as_ref() != Some(scope) {
            return;
        }
        let items = self
            .attachments
            .iter()
            .filter(|attachment| {
                attachment.bytes.is_none()
                    && attachment
                        .stored
                        .as_ref()
                        .is_some_and(|stored| stored.digest() == digest)
            })
            .map(|attachment| RecalledAttachmentInput {
                id: attachment.id.clone(),
                name: attachment.name.clone(),
                mime_type: mime_type.as_str().to_owned(),
                bytes: bytes.to_vec(),
            })
            .collect::<Vec<_>>();
        if !items.is_empty() {
            self.spawn_attachment_work(AttachmentWork::Recalled { items }, cx);
        }
    }
}
