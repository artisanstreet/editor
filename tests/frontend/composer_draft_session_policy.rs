//! Dependency-free coverage for composer draft session custody.

#[path = "../../modules/frontend/src/composer_draft_session_policy.rs"]
mod composer_draft_session_policy;

use composer_draft_session_policy::{
    ComposerDraft, ComposerDraftDocument, ComposerDraftSession, ComposerDraftStore,
    ComposerDraftToken, ComposerDraftWriteResult, ComposerImageAttachment,
    InMemoryComposerDraftStore,
};

fn document(text: &str, tokens: &[(&str, usize)]) -> ComposerDraftDocument {
    ComposerDraftDocument::new(
        text,
        tokens
            .iter()
            .map(|(id, position)| ComposerDraftToken::new(*id, *position))
            .collect(),
    )
}

fn attachment(id: &str, ready: bool) -> ComposerImageAttachment {
    let mut attachment = ComposerImageAttachment::new(id, ready);
    attachment.name = format!("{id}.png");
    attachment.preview_url = format!("blob:{id}");
    attachment
}

fn stored_draft(text: &str, attachments: &[ComposerImageAttachment]) -> ComposerDraft {
    ComposerDraft::new(document(text, &[]), attachments.to_vec())
}

fn ids(attachments: &[ComposerImageAttachment]) -> Vec<&str> {
    attachments
        .iter()
        .map(|attachment| attachment.id.as_str())
        .collect()
}

struct PlannedEvictionStore {
    evicted: Vec<ComposerDraft>,
    writes: Vec<ComposerDraft>,
}

impl PlannedEvictionStore {
    fn new(evicted: Vec<ComposerDraft>) -> Self {
        Self {
            evicted,
            writes: Vec::new(),
        }
    }
}

impl ComposerDraftStore for PlannedEvictionStore {
    fn read(&mut self, _draft_key: &str) -> Option<ComposerDraft> {
        None
    }

    fn write(&mut self, _draft_key: &str, draft: ComposerDraft) -> ComposerDraftWriteResult {
        self.writes.push(draft);
        ComposerDraftWriteResult {
            evicted: std::mem::take(&mut self.evicted),
        }
    }

    fn retained_attachment_ids(&self, _attachment_ids: &[String]) -> Vec<String> {
        Vec::new()
    }

    fn clear(&mut self, _draft_key: &str) {}
}

#[test]
fn absent_key_is_a_noop_for_persist_restore_and_clear() {
    let mut store = InMemoryComposerDraftStore::new();
    let existing = stored_draft("other", &[attachment("kept", true)]);
    let _ = store.write("other", existing.clone());
    let mut session = ComposerDraftSession::new(None);

    assert_eq!(session.draft_key(), None);
    assert_eq!(
        session.persist(
            &mut store,
            &document("ignored", &[("ignored", 1)]),
            &[attachment("ignored", true)],
        ),
        Default::default()
    );
    assert_eq!(session.restore(&mut store, true), Default::default());
    assert!(!session.restore_attempted());
    assert!(!session.clear(&mut store));
    assert_eq!(store.get("other"), Some(&existing));
}

#[test]
fn missing_target_does_not_consume_the_restore_opportunity() {
    let mut store = InMemoryComposerDraftStore::new();
    let saved = stored_draft("saved", &[attachment("ready", true)]);
    let _ = store.write("draft", saved.clone());
    let mut session = ComposerDraftSession::for_key("draft");

    assert_eq!(session.draft_key(), Some("draft"));
    assert_eq!(session.restore(&mut store, false), Default::default());
    assert!(!session.restore_attempted());

    let restored = session.restore(&mut store, true);
    assert!(restored.attempted);
    assert!(restored.restored.is_some());
    assert!(restored.persisted);
    assert!(session.restore_attempted());
    assert_eq!(session.restore(&mut store, true), Default::default());
}

#[test]
fn first_available_target_consumes_restore_even_when_store_is_empty() {
    let mut store = InMemoryComposerDraftStore::new();
    let mut session = ComposerDraftSession::for_key("draft");

    assert!(store.is_empty());

    let first = session.restore(&mut store, true);
    assert!(first.attempted);
    assert_eq!(first.restored, None);
    assert!(!first.persisted);

    let _ = store.write("draft", stored_draft("late", &[attachment("late", true)]));
    assert_eq!(session.restore(&mut store, true), Default::default());
}

#[test]
fn restore_releases_not_ready_values_and_persists_only_ready_values() {
    let mut store = InMemoryComposerDraftStore::new();
    let saved = ComposerDraft {
        text: "hello".into(),
        tokens: vec![
            ComposerDraftToken::new("pending", 1),
            ComposerDraftToken::new("ready", 3),
        ],
        attachments: vec![
            attachment("pending", false),
            attachment("ready", true),
            attachment("second-pending", false),
        ],
    };
    let _ = store.write("draft", saved.clone());
    let mut session = ComposerDraftSession::for_key("draft");

    let result = session.restore(&mut store, true);
    let restored = result.restored.expect("stored draft should be returned");
    assert_eq!(restored.document, saved.document());
    assert_eq!(ids(&restored.attachments), vec!["ready"]);
    assert_eq!(ids(&result.released), vec!["pending", "second-pending"]);
    assert!(result.persisted);
    assert_eq!(
        store.get("draft").map(|draft| ids(&draft.attachments)),
        Some(vec!["ready"])
    );
    assert_eq!(
        store.get("draft").map(|draft| draft.document()),
        Some(saved.document())
    );
}

#[test]
fn duplicate_restore_ids_keep_first_position_last_ready_value_and_one_release() {
    let mut store = InMemoryComposerDraftStore::new();
    let mut first_ready = attachment("duplicate", true);
    first_ready.preview_url = "blob:first".into();
    let mut last_ready = attachment("duplicate", true);
    last_ready.preview_url = "blob:last".into();
    let mut first_pending = attachment("pending", false);
    first_pending.preview_url = "blob:pending-first".into();
    let mut second_pending = attachment("pending", false);
    second_pending.preview_url = "blob:pending-second".into();
    let saved = ComposerDraft {
        text: "duplicates".into(),
        tokens: vec![
            ComposerDraftToken::new("duplicate", 0),
            ComposerDraftToken::new("other", 2),
            ComposerDraftToken::new("duplicate", 4),
        ],
        attachments: vec![
            first_ready,
            attachment("other", true),
            last_ready,
            first_pending,
            second_pending,
        ],
    };
    let _ = store.write("draft", saved);
    let mut session = ComposerDraftSession::for_key("draft");

    let result = session.restore(&mut store, true);
    let restored = result.restored.expect("stored draft should be returned");
    assert_eq!(ids(&restored.attachments), vec!["duplicate", "other"]);
    assert_eq!(restored.attachments[0].preview_url, "blob:last");
    assert_eq!(ids(&result.released), vec!["pending"]);
    assert_eq!(
        store.get("draft").map(|draft| ids(&draft.attachments)),
        Some(vec!["duplicate", "other"])
    );
}

#[test]
fn persist_copies_current_document_and_attachment_values() {
    let mut store = InMemoryComposerDraftStore::new();
    let session = ComposerDraftSession::for_key("draft");
    let mut current_document = document("before", &[("image", 2)]);
    let mut current_attachment = attachment("image", true);
    current_attachment.preview_url = "blob:before".into();

    let result = session.persist(&mut store, &current_document, &[current_attachment.clone()]);
    assert!(result.persisted);

    current_document.text = "after".into();
    current_attachment.preview_url = "blob:after".into();
    let saved = store.get("draft").expect("persisted draft");
    assert_eq!(saved.text, "before");
    assert_eq!(saved.attachments[0].preview_url, "blob:before");
}

#[test]
fn eviction_release_preserves_order_filters_live_ids_and_deduplicates_history() {
    let mut first_old = attachment("old", true);
    first_old.preview_url = "blob:first-old".into();
    let mut later_old = attachment("old", true);
    later_old.preview_url = "blob:later-old".into();
    let first = ComposerDraft {
        text: "first".into(),
        attachments: vec![first_old.clone(), attachment("active", true)],
        ..ComposerDraft::default()
    };
    let second = ComposerDraft {
        text: "second".into(),
        attachments: vec![
            later_old,
            attachment("second", true),
            attachment("active", true),
        ],
        ..ComposerDraft::default()
    };
    let mut store = PlannedEvictionStore::new(vec![first, second]);
    let session = ComposerDraftSession::for_key("current");

    let result = session.persist(
        &mut store,
        &document("current", &[]),
        &[attachment("active", true), attachment("live", true)],
    );

    assert_eq!(result.evicted.len(), 2);
    assert_eq!(ids(&result.released), vec!["old", "second"]);
    assert_eq!(result.released[0].preview_url, "blob:first-old");
    assert_eq!(store.writes.len(), 1);
    assert_eq!(store.writes[0].text, "current");
}

#[test]
fn eviction_does_not_release_an_id_in_the_just_written_live_set() {
    let mut store = InMemoryComposerDraftStore::with_maximum_drafts(1);
    let _ = store.write(
        "old",
        stored_draft(
            "old",
            &[attachment("shared", true), attachment("old", true)],
        ),
    );
    let session = ComposerDraftSession::for_key("current");

    let result = session.persist(
        &mut store,
        &document("current", &[]),
        &[attachment("shared", true)],
    );

    assert_eq!(ids(&result.released), vec!["old"]);
}

#[test]
fn release_unretained_checks_all_keys_and_preserves_live_order() {
    let mut store = InMemoryComposerDraftStore::new();
    let _ = store.write(
        "other",
        stored_draft("other", &[attachment("shared", true)]),
    );
    let session = ComposerDraftSession::for_key("current");
    let live = vec![
        attachment("shared", true),
        attachment("orphan", true),
        attachment("orphan", true),
    ];

    assert_eq!(
        ids(&session.release_unretained(&store, &live)),
        vec!["orphan"]
    );

    store.clear("other");
    assert_eq!(
        ids(&session.release_unretained(&store, &live)),
        vec!["shared", "orphan"]
    );
}

#[test]
fn clear_removes_only_the_current_key_and_does_not_release_values() {
    let mut store = InMemoryComposerDraftStore::new();
    let _ = store.write(
        "current",
        stored_draft("current", &[attachment("one", true)]),
    );
    let other = stored_draft("other", &[attachment("two", true)]);
    let _ = store.write("other", other.clone());
    let session = ComposerDraftSession::for_key("current");

    assert!(session.clear(&mut store));
    assert!(!store.contains("current"));
    assert_eq!(store.get("other"), Some(&other));
    assert_eq!(
        session
            .release_unretained(&store, &[attachment("one", true)])
            .len(),
        1
    );
}

#[test]
fn in_memory_store_reads_refresh_lru_but_retention_is_cross_key() {
    let mut store = InMemoryComposerDraftStore::with_maximum_drafts(2);
    assert_eq!(store.maximum_drafts(), 2);
    let _ = store.write("a", stored_draft("a", &[attachment("a", true)]));
    let _ = store.write("b", stored_draft("b", &[attachment("b", true)]));
    assert_eq!(store.read("a"), store.get("a").cloned());
    assert_eq!(store.ordered_keys(), vec!["b", "a"]);

    let result = store.write("c", stored_draft("c", &[attachment("c", true)]));
    assert_eq!(result.evicted.len(), 1);
    assert_eq!(result.evicted[0].attachments[0].id, "b");
    assert_eq!(store.len(), 2);
    assert_eq!(
        store.retained_attachment_ids(&["a".into(), "b".into(), "c".into()]),
        vec!["a", "c"]
    );
}
