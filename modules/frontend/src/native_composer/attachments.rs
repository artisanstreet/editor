//! Attachment intake, preview, and delivery state for the native composer.
//!
//! Extracted verbatim from `native_composer.rs` during the module split;
//! visibility was widened to `pub(super)` for parent-owned state and actions.

#![forbid(unsafe_code)]

use super::*;

pub(super) enum AttachmentWork {
    Clipboard {
        items: Vec<(String, ClipboardImageCandidate)>,
    },
    Files {
        items: Vec<(String, std::path::PathBuf)>,
    },
    Restored {
        items: Vec<RestoredAttachmentInput>,
    },
    Recalled {
        items: Vec<RecalledAttachmentInput>,
    },
}

#[derive(Clone)]
pub(super) struct PasteScope {
    pub(super) thread: Option<String>,
    pub(super) draft_generation: u64,
    pub(super) selection_revision: u64,
    pub(super) replacement_range: Option<Range<usize>>,
}

pub(super) struct AttachmentPreviewState {
    pub(super) attachment_id: String,
    pub(super) draft_generation: u64,
    pub(super) request: u64,
    pub(super) image: Arc<RenderImage>,
}

struct PreviewWork {
    format: ImageFormat,
    bytes: Arc<Vec<u8>>,
}

impl PreviewWork {
    fn run(self) -> Result<Arc<RenderImage>, AttachmentPreparationError> {
        render_full_preview(self.format, self.bytes.as_ref())
    }
}

impl AttachmentWork {
    fn run(self) -> Vec<AttachmentPreparationOutcome> {
        match self {
            Self::Clipboard { items } => prepare_clipboard_batch(items, None),
            Self::Files { items } => prepare_file_batch(items, None),
            Self::Restored { items } => prepare_restored_batch(items, None),
            Self::Recalled { items } => prepare_recalled_batch(items),
        }
    }
}

impl NativeComposer {
    pub(super) fn next_unique_attachment_id(
        &mut self,
        reserved: &mut HashSet<String>,
    ) -> Option<String> {
        for _ in 0..=MAXIMUM_ATTACHMENT_COUNT {
            let id = self.next_attachment_id();
            if !self
                .attachments
                .iter()
                .any(|attachment| attachment.id == id)
                && reserved.insert(id.clone())
            {
                return Some(id);
            }
        }
        None
    }

    #[cfg(test)]
    /// Returns the number of live attachment slots, including pending reads.
    pub(crate) fn attachment_count(&self) -> usize {
        self.attachments.len()
    }

    /// Snapshots ready images in the exact tray order for a future typed
    /// message-content command.
    ///
    /// No text marker or placeholder is created. A pending image is an
    /// explicit refusal until its encoded bytes and tray thumbnail are ready.
    pub(crate) fn snapshot_ordered_ready_attachments(
        &self,
    ) -> Result<NativeComposerAttachmentSnapshot, AttachmentPayloadError> {
        let attachments = self
            .attachments
            .iter()
            .enumerate()
            .map(|(position, attachment)| attachment.payload(position))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(NativeComposerAttachmentSnapshot {
            draft_generation: self.draft_generation,
            attachments,
        })
    }

    #[cfg(test)]
    /// Returns a fresh payload for a retry only when the complete ordered
    /// attachment set still matches the original generation and bytes.
    pub(crate) fn match_retry_attachment_payload(
        &self,
        snapshot: &NativeComposerAttachmentSnapshot,
    ) -> Option<NativeComposerAttachmentSnapshot> {
        let current = self.snapshot_ordered_ready_attachments().ok()?;
        (current == *snapshot).then_some(current)
    }

    /// Enables the future typed attachment submission path. The current
    /// `QueueFirstMessage` caller deliberately does not invoke this seam.
    pub(crate) fn set_attachment_delivery_enabled(
        &mut self,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        if self.attachment_delivery_enabled != enabled {
            self.attachment_delivery_enabled = enabled;
            cx.notify();
        }
    }

    pub(super) fn clear_attachment_preview(&mut self) {
        self.attachment_preview_request = self.attachment_preview_request.saturating_add(1);
        self.attachment_preview = None;
        self.attachment_preview_error = None;
        self.attachment_preview_task = None;
    }

    fn next_attachment_id(&mut self) -> String {
        let id = format!("attachment:{}", self.next_attachment_id);
        self.next_attachment_id = self.next_attachment_id.saturating_add(1);
        id
    }

    fn current_pending_input_total(&self) -> usize {
        self.attachments
            .iter()
            .filter(|attachment| !attachment.is_ready())
            .fold(0usize, |total, attachment| {
                total.saturating_add(attachment.source_bytes_len())
            })
    }

    pub(super) fn enqueue_clipboard_images(
        &mut self,
        candidates: Vec<ClipboardImageCandidate>,
        cx: &mut Context<Self>,
    ) {
        self.attachment_error = None;
        let mut pending_input_total = self.current_pending_input_total();
        let mut work = Vec::new();
        for candidate in candidates {
            if !matches!(
                candidate.format,
                ImageFormat::Gif | ImageFormat::Jpeg | ImageFormat::Png | ImageFormat::Webp
            ) {
                self.attachment_error =
                    Some(native_composer_attachments::ATTACHMENT_UNSUPPORTED_FORMAT_MESSAGE.into());
                continue;
            }
            if candidate.bytes.len() > MAXIMUM_RAW_ATTACHMENT_BYTES {
                self.attachment_error =
                    Some(native_composer_attachments::ATTACHMENT_TOO_LARGE_MESSAGE.into());
                continue;
            }
            if self.attachments.len() >= MAXIMUM_ATTACHMENT_COUNT {
                self.attachment_error =
                    Some(native_composer_attachments::ATTACHMENT_COUNT_LIMIT_MESSAGE.into());
                break;
            }
            let incoming_size = candidate.bytes.len();
            if pending_input_total.saturating_add(incoming_size)
                > MAXIMUM_RAW_ATTACHMENT_TOTAL_BYTES
            {
                self.attachment_error =
                    Some(native_composer_attachments::ATTACHMENT_TOTAL_LIMIT_MESSAGE.into());
                continue;
            }

            let id = self.next_attachment_id();
            let name = candidate.name.clone();
            self.attachments.push(ComposerAttachment::pending(
                id.clone(),
                name,
                Some(candidate.format),
                candidate.format.mime_type(),
                incoming_size,
            ));
            pending_input_total = pending_input_total.saturating_add(incoming_size);
            work.push((id, candidate));
        }

        if !work.is_empty() {
            self.spawn_attachment_work(AttachmentWork::Clipboard { items: work }, cx);
            self.persist_current_draft();
            cx.notify();
        }
    }

    pub(super) fn enqueue_file_drop(
        &mut self,
        paths: Vec<std::path::PathBuf>,
        cx: &mut Context<Self>,
    ) {
        self.attachment_error = None;
        if paths.is_empty() {
            return;
        }
        if self.attachments.len().saturating_add(paths.len()) > MAXIMUM_ATTACHMENT_COUNT {
            self.attachment_error =
                Some(native_composer_attachments::ATTACHMENT_COUNT_LIMIT_MESSAGE.into());
            cx.notify();
            return;
        }

        let mut work = Vec::with_capacity(paths.len());
        for path in paths {
            let id = self.next_attachment_id();
            let name = display_file_name(&path);
            self.attachments
                .push(ComposerAttachment::pending(id.clone(), name, None, "", 0));
            work.push((id, path));
        }
        self.spawn_attachment_work(AttachmentWork::Files { items: work }, cx);
        self.persist_current_draft();
        cx.notify();
    }

    pub(super) fn spawn_attachment_work(&mut self, work: AttachmentWork, cx: &mut Context<Self>) {
        // Task handles cancel their futures when dropped. Completed handles do
        // not need to remain in the composer, so retire them before every new
        // piece of attachment work. The draft generation and attachment IDs
        // below remain the authoritative completion fences for work that is
        // still running.
        self.prune_attachment_tasks();
        let generation = self.draft_generation;
        let thread = self.draft_thread.clone();
        let task = cx.spawn(async move |this, cx| {
            let outcomes = cx
                .background_executor()
                .spawn(async move { work.run() })
                .await;
            this.update(cx, |composer, composer_cx| {
                composer.apply_attachment_outcomes(
                    thread.as_deref(),
                    generation,
                    outcomes,
                    composer_cx,
                );
            })
            .ok();
        });
        self.attachment_tasks.push(task);
    }

    fn apply_attachment_outcomes(
        &mut self,
        thread: Option<&str>,
        generation: u64,
        outcomes: Vec<AttachmentPreparationOutcome>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.draft_generation || thread != self.draft_thread.as_deref() {
            self.prune_attachment_tasks();
            return;
        }

        let mut changed = false;
        for outcome in outcomes {
            let Some(index) = self
                .attachments
                .iter()
                .position(|attachment| attachment.id == outcome.id)
            else {
                continue;
            };
            changed = true;
            match outcome.result {
                Ok(prepared) => {
                    if self.attachment_is_duplicate(index, &prepared) {
                        self.attachments.remove(index);
                        self.attachment_error = Some("That image is already attached.".into());
                        continue;
                    }
                    let other_total = self
                        .attachments
                        .iter()
                        .enumerate()
                        .filter(|(other_index, attachment)| {
                            *other_index != index && attachment.is_ready()
                        })
                        .fold(0usize, |total, (_, attachment)| {
                            total.saturating_add(attachment.size_bytes)
                        });
                    if other_total.saturating_add(prepared.size_bytes)
                        > MAXIMUM_ATTACHMENT_TOTAL_BYTES
                    {
                        self.attachments.remove(index);
                        self.attachment_error = Some(
                            native_composer_attachments::ATTACHMENT_TOTAL_LIMIT_MESSAGE.into(),
                        );
                        continue;
                    }
                    self.attachments[index] = ComposerAttachment::from_prepared(prepared);
                }
                Err(error) => {
                    self.attachments.remove(index);
                    self.attachment_error = Some(format!("{}: {error}", outcome.name));
                }
            }
        }
        if changed {
            self.viewed_attachment = self.viewed_attachment.take().filter(|id| {
                self.attachments
                    .iter()
                    .any(|attachment| &attachment.id == id)
            });
            self.persist_current_draft();
            cx.notify();
        }
        self.prune_attachment_tasks();
    }

    fn attachment_is_duplicate(&self, index: usize, prepared: &PreparedComposerAttachment) -> bool {
        self.attachments
            .iter()
            .enumerate()
            .filter(|(other_index, _)| *other_index != index)
            .any(|(_, attachment)| {
                (attachment.is_ready()
                    && attachment.source_digest == prepared.source_digest
                    && attachment.source_size_bytes == prepared.source_size_bytes)
                    || (attachment.is_ready()
                        && attachment.encoded_digest == prepared.encoded_digest
                        && attachment.size_bytes == prepared.size_bytes)
            })
    }

    pub(super) fn remove_attachment(&mut self, attachment_id: &str, cx: &mut Context<Self>) {
        self.prune_attachment_tasks();
        let Some(index) = self
            .attachments
            .iter()
            .position(|attachment| attachment.id == attachment_id)
        else {
            return;
        };
        self.attachments.remove(index);
        if self.viewed_attachment.as_deref() == Some(attachment_id) {
            self.viewed_attachment = None;
            self.clear_attachment_preview();
        }
        self.attachment_error = None;
        self.persist_current_draft();
        cx.notify();
    }

    pub(super) fn view_attachment(&mut self, attachment_id: &str, cx: &mut Context<Self>) {
        let Some(attachment) = self
            .attachments
            .iter()
            .find(|attachment| attachment.id == attachment_id && attachment.is_ready())
        else {
            return;
        };
        let Some(format) = attachment.format else {
            return;
        };
        let Some(bytes) = attachment.bytes.clone() else {
            return;
        };

        self.viewed_attachment = Some(attachment_id.to_owned());
        self.clear_attachment_preview();
        let request = self.attachment_preview_request;
        let generation = self.draft_generation;
        let id = attachment_id.to_owned();
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { PreviewWork { format, bytes }.run() })
                .await;
            this.update(cx, |composer, composer_cx| {
                if composer.draft_generation != generation
                    || composer.attachment_preview_request != request
                    || composer.viewed_attachment.as_deref() != Some(id.as_str())
                {
                    return;
                }
                composer.attachment_preview_task = None;
                match result {
                    Ok(image) => {
                        composer.attachment_preview = Some(AttachmentPreviewState {
                            attachment_id: id,
                            draft_generation: generation,
                            request,
                            image,
                        });
                    }
                    Err(error) => {
                        composer.attachment_preview_error = Some(error.to_string());
                    }
                }
                composer_cx.notify();
            })
            .ok();
        });
        self.attachment_preview_task = Some(task);
        cx.notify();
    }

    pub(super) fn close_attachment_viewer(&mut self, cx: &mut Context<Self>) {
        if self.viewed_attachment.take().is_some() {
            self.clear_attachment_preview();
            cx.notify();
        }
    }

    pub(super) fn clear_submitted_attachment_values(
        &mut self,
        snapshot: &NativeComposerAttachmentSnapshot,
    ) -> bool {
        if !self.attachment_snapshot_matches_current(snapshot) {
            return false;
        }
        let ids = snapshot
            .attachments
            .iter()
            .map(|attachment| attachment.client_token.as_str())
            .collect::<std::collections::HashSet<_>>();
        if ids.is_empty() {
            return true;
        }
        if self
            .viewed_attachment
            .as_deref()
            .is_some_and(|id| ids.contains(id))
        {
            self.viewed_attachment = None;
            self.clear_attachment_preview();
        }
        self.attachments
            .retain(|attachment| !ids.contains(attachment.id.as_str()));
        self.persist_current_draft();
        true
    }

    fn attachment_snapshot_matches_current(
        &self,
        snapshot: &NativeComposerAttachmentSnapshot,
    ) -> bool {
        if snapshot.draft_generation != self.draft_generation {
            return false;
        }
        snapshot.attachments.iter().all(|expected| {
            let Some((position, current)) = self
                .attachments
                .iter()
                .enumerate()
                .find(|(_, attachment)| attachment.id == expected.client_token)
            else {
                return false;
            };
            position == expected.position
                && current
                    .payload(position)
                    .is_ok_and(|payload| payload == expected.clone())
        })
    }
}
