//! Deterministic, DOM-free custody for one composer draft session.
//!
//! This is the native counterpart of
//! `routes/components/composer/draft-session.ts`.  It owns the session's
//! one-shot restore fence and decides which attachment values may be released;
//! it does not read an editor, invoke an Effect runtime, revoke a browser URL,
//! or choose a persistence implementation.  A host supplies the current
//! document snapshot and an implementation of [`ComposerDraftStore`], then
//! acts on the returned release values.

#![forbid(unsafe_code)]

use std::collections::{HashMap, HashSet, VecDeque};

/// One inline image marker's identity and position in a draft document.
///
/// Positions are already in the caller's editor coordinate system.  This
/// leaf preserves them verbatim; DOM-specific clamping and marker rendering
/// belong to the host boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComposerDraftToken {
    /// Stable attachment identity referenced by the marker.
    pub id: String,
    /// Position of the marker in the document text.
    pub position: usize,
}

impl ComposerDraftToken {
    /// Creates an owned token snapshot.
    #[must_use]
    pub fn new(id: impl Into<String>, position: usize) -> Self {
        Self {
            id: id.into(),
            position,
        }
    }
}

/// Opaque attachment data held by a composer draft.
///
/// The fields mirror the attachment value passed between the TypeScript
/// composer and draft store.  The session treats them as owned data: in
/// particular, `preview_url` is never interpreted or revoked here.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ComposerImageAttachment {
    /// Encoded image content, when [`Self::ready`] is true.
    pub content_base64: String,
    /// Stable identity used by tokens and custody decisions.
    pub id: String,
    /// Opaque MIME type retained with the attachment.
    pub mime_type: String,
    /// Source file name retained for the host/UI.
    pub name: String,
    /// Opaque host-owned preview value.
    pub preview_url: String,
    /// Whether this attachment can be revived for the editor.
    pub ready: bool,
    /// Encoded image size in bytes.
    pub size_bytes: usize,
    /// Digest of the original source bytes.
    pub source_digest: String,
    /// Original source size in bytes.
    pub source_size_bytes: usize,
}

impl ComposerImageAttachment {
    /// Creates a compact attachment fixture with an explicit readiness bit.
    ///
    /// Other fields intentionally start empty because this policy never
    /// decodes or validates image data.  Callers that need those values can
    /// fill the public fields before storing the snapshot.
    #[must_use]
    pub fn new(id: impl Into<String>, ready: bool) -> Self {
        Self {
            id: id.into(),
            ready,
            ..Self::default()
        }
    }
}

/// The editor document snapshot supplied to a session operation.
///
/// This record intentionally contains no DOM node or editor handle.  The
/// caller reads the editor once and supplies the resulting owned values to
/// [`ComposerDraftSession::persist`].
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ComposerDraftDocument {
    /// Verbatim visible editor text.
    pub text: String,
    /// Inline image markers in editor order.
    pub tokens: Vec<ComposerDraftToken>,
}

impl ComposerDraftDocument {
    /// Creates an owned document snapshot.
    #[must_use]
    pub fn new(text: impl Into<String>, tokens: Vec<ComposerDraftToken>) -> Self {
        Self {
            text: text.into(),
            tokens,
        }
    }
}

/// A complete stored composer draft: document plus attachment values.
///
/// Attachment order is meaningful.  It is the order exposed by the live
/// attachment map when the draft is persisted and the order used to select
/// historical release values.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ComposerDraft {
    /// Attachment values owned by this stored draft.
    pub attachments: Vec<ComposerImageAttachment>,
    /// Verbatim editor text.
    pub text: String,
    /// Inline image markers in document order.
    pub tokens: Vec<ComposerDraftToken>,
}

impl ComposerDraft {
    /// Creates a complete stored draft from a document and attachment values.
    #[must_use]
    pub fn new(document: ComposerDraftDocument, attachments: Vec<ComposerImageAttachment>) -> Self {
        Self {
            attachments,
            text: document.text,
            tokens: document.tokens,
        }
    }

    /// Returns the document portion as a new owned snapshot.
    #[must_use]
    pub fn document(&self) -> ComposerDraftDocument {
        ComposerDraftDocument {
            text: self.text.clone(),
            tokens: self.tokens.clone(),
        }
    }
}

/// Result returned by a store write.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ComposerDraftWriteResult {
    /// Drafts removed by the store, in its stable eviction order.
    pub evicted: Vec<ComposerDraft>,
}

/// A synchronous store seam owned by the embedding host.
///
/// `read` may refresh a store's recency order, as the browser draft store does.
/// The session never assumes a particular capacity, serialization format, or
/// backing system.  `retained_attachment_ids` is queried across every key so
/// unmount cleanup can honor drafts belonging to another composer instance.
pub trait ComposerDraftStore {
    /// Reads one draft, returning an owned snapshot when the key exists.
    fn read(&mut self, draft_key: &str) -> Option<ComposerDraft>;

    /// Writes one draft and reports values displaced by store retention.
    fn write(&mut self, draft_key: &str, draft: ComposerDraft) -> ComposerDraftWriteResult;

    /// Returns live attachment IDs retained by any draft in the store.
    fn retained_attachment_ids(&self, attachment_ids: &[String]) -> Vec<String>;

    /// Removes one key without releasing any attachment value.
    fn clear(&mut self, draft_key: &str);
}

/// A small deterministic in-memory implementation of [`ComposerDraftStore`].
///
/// Entries are ordered least-recently-used first.  Reads and writes move a key
/// to the back.  The newest write is protected from eviction even when the
/// configured capacity is too small, matching the legacy store's protection
/// of the just-written draft.  This type is only a testable synchronous seam;
/// it performs no URL or filesystem cleanup.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InMemoryComposerDraftStore {
    drafts: VecDeque<RetainedDraft>,
    maximum_drafts: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RetainedDraft {
    key: String,
    draft: ComposerDraft,
}

/// Default number of drafts retained by the in-memory seam.
pub const DEFAULT_MAXIMUM_COMPOSER_DRAFTS: usize = 6;

impl Default for InMemoryComposerDraftStore {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryComposerDraftStore {
    /// Creates an in-memory store with the legacy six-draft capacity.
    #[must_use]
    pub fn new() -> Self {
        Self::with_maximum_drafts(DEFAULT_MAXIMUM_COMPOSER_DRAFTS)
    }

    /// Creates an in-memory store with a caller-selected draft capacity.
    ///
    /// A zero capacity is accepted for boundary tests, but the newest write
    /// is still retained because the just-written draft is always protected.
    #[must_use]
    pub const fn with_maximum_drafts(maximum_drafts: usize) -> Self {
        Self {
            drafts: VecDeque::new(),
            maximum_drafts,
        }
    }

    /// Returns the number of currently retained drafts.
    #[must_use]
    pub fn len(&self) -> usize {
        self.drafts.len()
    }

    /// Returns whether no draft is retained.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.drafts.is_empty()
    }

    /// Returns the configured capacity.
    #[must_use]
    pub const fn maximum_drafts(&self) -> usize {
        self.maximum_drafts
    }

    /// Returns whether a key exists without changing recency.
    #[must_use]
    pub fn contains(&self, draft_key: &str) -> bool {
        self.drafts.iter().any(|retained| retained.key == draft_key)
    }

    /// Returns a retained draft without changing recency.
    #[must_use]
    pub fn get(&self, draft_key: &str) -> Option<&ComposerDraft> {
        self.drafts
            .iter()
            .find(|retained| retained.key == draft_key)
            .map(|retained| &retained.draft)
    }

    /// Returns keys from oldest to newest.
    #[must_use]
    pub fn ordered_keys(&self) -> Vec<String> {
        self.drafts
            .iter()
            .map(|retained| retained.key.clone())
            .collect()
    }

    fn clear_key(&mut self, draft_key: &str) {
        if let Some(index) = self
            .drafts
            .iter()
            .position(|retained| retained.key == draft_key)
        {
            self.drafts.remove(index);
        }
    }
}

impl ComposerDraftStore for InMemoryComposerDraftStore {
    fn read(&mut self, draft_key: &str) -> Option<ComposerDraft> {
        let index = self
            .drafts
            .iter()
            .position(|retained| retained.key == draft_key)?;
        let retained = self
            .drafts
            .remove(index)
            .expect("draft index came from the retained draft list");
        let draft = retained.draft.clone();
        self.drafts.push_back(retained);
        Some(draft)
    }

    fn write(&mut self, draft_key: &str, draft: ComposerDraft) -> ComposerDraftWriteResult {
        // The legacy store treats an empty document as a clear, even if a
        // caller supplied unmatched token metadata.
        if draft.text.is_empty() && draft.attachments.is_empty() {
            self.clear_key(draft_key);
            return ComposerDraftWriteResult::default();
        }

        self.clear_key(draft_key);
        self.drafts.push_back(RetainedDraft {
            key: draft_key.to_owned(),
            draft,
        });

        let mut evicted = Vec::new();
        while self.drafts.len() > self.maximum_drafts {
            let newest_is_oldest = self
                .drafts
                .front()
                .is_some_and(|retained| retained.key == draft_key);
            if newest_is_oldest {
                break;
            }

            let retained = self
                .drafts
                .pop_front()
                .expect("capacity pressure requires a retained draft");
            evicted.push(retained.draft);
        }

        ComposerDraftWriteResult { evicted }
    }

    fn retained_attachment_ids(&self, attachment_ids: &[String]) -> Vec<String> {
        let requested = attachment_ids
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        let mut retained = Vec::new();
        let mut seen = HashSet::new();

        for draft in self.drafts.iter().map(|retained| &retained.draft) {
            for attachment in &draft.attachments {
                if requested.contains(attachment.id.as_str()) && seen.insert(&attachment.id) {
                    retained.push(attachment.id.clone());
                }
            }
        }

        retained
    }

    fn clear(&mut self, draft_key: &str) {
        self.clear_key(draft_key);
    }
}

/// Selects evicted attachment values that the current live set does not own.
///
/// The traversal preserves evicted-draft order and attachment order.  A
/// repeated ID contributes only its first value, and an ID in the just-written
/// live set is skipped even when an evicted historical draft also contains it.
#[must_use]
pub fn select_evicted_attachments_to_release(
    evicted: &[ComposerDraft],
    active_attachment_ids: &[String],
) -> Vec<ComposerImageAttachment> {
    let active_ids = active_attachment_ids
        .iter()
        .cloned()
        .collect::<HashSet<_>>();
    let mut selected_ids = HashSet::new();
    let mut selected = Vec::new();

    for draft in evicted {
        for attachment in &draft.attachments {
            if active_ids.contains(&attachment.id) || !selected_ids.insert(attachment.id.clone()) {
                continue;
            }
            selected.push(attachment.clone());
        }
    }

    selected
}

/// Explicit result of a persist operation.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ComposerDraftPersistResult {
    /// Whether a keyed store write was performed.  An empty write may clear
    /// the key inside the store while still counting as an operation.
    pub persisted: bool,
    /// Drafts displaced by that write.
    pub evicted: Vec<ComposerDraft>,
    /// Attachment values whose host-owned resources should be released.
    pub released: Vec<ComposerImageAttachment>,
}

/// The DOM-free value delivered to a host after a successful draft restore.
///
/// `document` is the stored snapshot passed to the restoration boundary and
/// `attachments` contains only ready values, keyed by ID with JavaScript Map
/// order: the first occurrence fixes a position and a later ready duplicate
/// replaces that position's value.  No DOM marker filtering or position
/// clamping is performed here.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComposerDraftRestoration {
    /// Stored document snapshot to apply at the host boundary.
    pub document: ComposerDraftDocument,
    /// Ready attachment values in deterministic keyed insertion order.
    pub attachments: Vec<ComposerImageAttachment>,
}

/// Explicit result of one restore opportunity.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ComposerDraftRestoreResult {
    /// Whether this call consumed the first non-null target opportunity.
    pub attempted: bool,
    /// Restored output, or `None` when the keyed store was empty.
    pub restored: Option<ComposerDraftRestoration>,
    /// Whether the revived state was written back to the store.
    pub persisted: bool,
    /// Drafts displaced by the post-restore write.
    pub evicted: Vec<ComposerDraft>,
    /// Not-ready historical values followed by any post-write release values,
    /// deduplicated by ID in first-seen order.
    pub released: Vec<ComposerImageAttachment>,
}

/// Synchronous controller for one composer session.
///
/// The controller stores only the immutable draft key and the one-shot restore
/// fence.  Current editor values are deliberately supplied to each persist or
/// unmount operation so this type never becomes a second source of truth for
/// UI state.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ComposerDraftSession {
    draft_key: Option<String>,
    restore_attempted: bool,
}

impl ComposerDraftSession {
    /// Creates a session for an optional draft key.
    #[must_use]
    pub fn new(draft_key: Option<String>) -> Self {
        Self {
            draft_key,
            restore_attempted: false,
        }
    }

    /// Creates a keyed session from any string-like key.
    #[must_use]
    pub fn for_key(draft_key: impl Into<String>) -> Self {
        Self::new(Some(draft_key.into()))
    }

    /// Returns the session's immutable key, if one was assigned.
    #[must_use]
    pub fn draft_key(&self) -> Option<&str> {
        self.draft_key.as_deref()
    }

    /// Returns whether a non-null target has already consumed restore.
    #[must_use]
    pub const fn restore_attempted(&self) -> bool {
        self.restore_attempted
    }

    /// Persists one current editor snapshot and returns custody work.
    ///
    /// A missing key is a complete no-op.  For a keyed session, the attachment
    /// slice is treated as the live map's value order and copied into the
    /// stored draft.  Evicted values are selected after the write, skipping
    /// live IDs and collapsing duplicate historical IDs.
    #[must_use]
    pub fn persist<S: ComposerDraftStore>(
        &self,
        store: &mut S,
        document: &ComposerDraftDocument,
        attachments: &[ComposerImageAttachment],
    ) -> ComposerDraftPersistResult {
        let Some(draft_key) = self.draft_key.as_deref() else {
            return ComposerDraftPersistResult::default();
        };

        let draft = ComposerDraft::new(document.clone(), attachments.to_vec());
        let write = store.write(draft_key, draft);
        let active_ids = attachments
            .iter()
            .map(|attachment| attachment.id.clone())
            .collect::<Vec<_>>();
        let released = select_evicted_attachments_to_release(&write.evicted, &active_ids);

        ComposerDraftPersistResult {
            persisted: true,
            evicted: write.evicted,
            released,
        }
    }

    /// Attempts restoration for an explicitly available host target.
    ///
    /// A null target does not consume the one-shot fence.  The first non-null
    /// target for a keyed session consumes it before the store read, including
    /// when the store is empty; later target opportunities are no-ops.  Every
    /// not-ready stored attachment is released once by ID, ready values are
    /// revived with Map-compatible keyed order, and that revived state is then
    /// persisted through the same custody path as an ordinary write.
    #[must_use]
    pub fn restore<S: ComposerDraftStore>(
        &mut self,
        store: &mut S,
        target_available: bool,
    ) -> ComposerDraftRestoreResult {
        if !target_available || self.restore_attempted || self.draft_key.is_none() {
            return ComposerDraftRestoreResult::default();
        }

        self.restore_attempted = true;
        let draft_key = self
            .draft_key
            .as_deref()
            .expect("the missing key was checked before consuming restore");
        let Some(stored) = store.read(draft_key) else {
            return ComposerDraftRestoreResult {
                attempted: true,
                ..ComposerDraftRestoreResult::default()
            };
        };

        let mut released = Vec::new();
        let mut released_ids = HashSet::new();
        for attachment in stored
            .attachments
            .iter()
            .filter(|attachment| !attachment.ready)
        {
            if released_ids.insert(attachment.id.clone()) {
                released.push(attachment.clone());
            }
        }

        let revived_attachments = revive_ready_attachments(&stored.attachments);
        let restoration = ComposerDraftRestoration {
            document: stored.document(),
            attachments: revived_attachments.clone(),
        };
        let persisted = self.persist(store, &restoration.document, &revived_attachments);
        append_unique_attachments(&mut released, persisted.released, &mut released_ids);

        ComposerDraftRestoreResult {
            attempted: true,
            restored: Some(restoration),
            persisted: persisted.persisted,
            evicted: persisted.evicted,
            released,
        }
    }

    /// Selects current live values not retained by any store key.
    ///
    /// This operation intentionally does not require a draft key: an
    /// attachment can be retained by another session's draft during a route
    /// switch.  Live input order is preserved and duplicate IDs are released
    /// at most once.
    #[must_use]
    pub fn release_unretained<S: ComposerDraftStore>(
        &self,
        store: &S,
        attachments: &[ComposerImageAttachment],
    ) -> Vec<ComposerImageAttachment> {
        let live_ids = attachments
            .iter()
            .map(|attachment| attachment.id.clone())
            .collect::<Vec<_>>();
        let retained = store
            .retained_attachment_ids(&live_ids)
            .into_iter()
            .collect::<HashSet<_>>();
        let mut released_ids = HashSet::new();
        let mut released = Vec::new();

        for attachment in attachments {
            if retained.contains(&attachment.id) || !released_ids.insert(attachment.id.clone()) {
                continue;
            }
            released.push(attachment.clone());
        }

        released
    }

    /// Removes the session's current key, if one exists.
    ///
    /// The boolean reports whether a keyed clear operation was issued.  Clear
    /// itself never returns attachment values; ownership remains with the
    /// caller, matching the browser session boundary.
    pub fn clear<S: ComposerDraftStore>(&self, store: &mut S) -> bool {
        let Some(draft_key) = self.draft_key.as_deref() else {
            return false;
        };
        store.clear(draft_key);
        true
    }
}

/// Revives ready values using JavaScript Map's insertion/update behavior.
fn revive_ready_attachments(
    attachments: &[ComposerImageAttachment],
) -> Vec<ComposerImageAttachment> {
    let mut positions = HashMap::new();
    let mut revived = Vec::new();

    for attachment in attachments.iter().filter(|attachment| attachment.ready) {
        if let Some(index) = positions.get(&attachment.id).copied() {
            revived[index] = attachment.clone();
        } else {
            positions.insert(attachment.id.clone(), revived.len());
            revived.push(attachment.clone());
        }
    }

    revived
}

/// Appends values whose IDs have not already occurred in a release plan.
fn append_unique_attachments(
    selected: &mut Vec<ComposerImageAttachment>,
    additions: Vec<ComposerImageAttachment>,
    selected_ids: &mut HashSet<String>,
) {
    for attachment in additions {
        if selected_ids.insert(attachment.id.clone()) {
            selected.push(attachment);
        }
    }
}
