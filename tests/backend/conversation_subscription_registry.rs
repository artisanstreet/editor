//! Direct state-table tests for the per-connection conversation subscription registry.

#![forbid(unsafe_code)]

#[path = "../../modules/backend/src/conversation_subscription_registry.rs"]
mod conversation_subscription_registry;

use artisan_domain::{
    ConversationCursor, ConversationPatch, IncrementalText, ItemId, PatchBatch, PatchId,
    PatchSequence, Revision, ThreadId, UnixMillis,
};
use conversation_subscription_registry::{
    ActivateError, ApplyBatchError, ConversationSubscriptionRegistry, RegisterError,
    SubscriptionState, UnsubscribeOutcome,
};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn thread(id: &str) -> ThreadId {
    ThreadId::parse(id).expect("fixture thread id should be valid")
}

fn cursor(value: u64) -> ConversationCursor {
    ConversationCursor::new(value)
}

fn append_patch(id: &str, sequence: u64) -> ConversationPatch {
    ConversationPatch::ItemAppend {
        patch_id: PatchId::parse(id).expect("fixture patch id should be valid"),
        sequence: PatchSequence::new(sequence).expect("fixture sequence should be positive"),
        item_id: ItemId::parse("item-1").expect("fixture item id should be valid"),
        revision: Revision::new(sequence),
        text: IncrementalText::parse("x").expect("fixture fragment should be valid"),
        updated_at: UnixMillis::from_millis(1),
    }
}

fn batch(thread_id: ThreadId, from: u64, to: u64, patches: Vec<ConversationPatch>) -> PatchBatch {
    PatchBatch::new(thread_id, cursor(from), cursor(to), patches)
        .expect("fixture batch should be valid")
}

// ---------------------------------------------------------------------------
// Empty / first lease
// ---------------------------------------------------------------------------

#[test]
fn empty_registry_has_no_entries_and_first_pending_lease_is_nonzero_at_cursor_zero() {
    let mut registry = ConversationSubscriptionRegistry::new();
    assert!(registry.is_empty());
    assert_eq!(registry.len(), 0);
    assert!(!registry.contains(&thread("thread-1")));
    assert!(registry.cursor(&thread("thread-1")).is_none());
    assert!(registry.state(&thread("thread-1")).is_none());
    assert!(registry.view(&thread("thread-1")).is_none());
    assert!(registry.get(&thread("thread-1")).is_none());
    assert_eq!(
        registry.unsubscribe(&thread("thread-1")),
        UnsubscribeOutcome::Absent
    );

    let lease = registry
        .register(thread("thread-1"), cursor(0))
        .expect("first registration should succeed");
    assert_eq!(lease.thread_id(), &thread("thread-1"));
    assert!(lease.generation().get() != 0);
    assert_eq!(lease.generation().get(), 1);
    assert!(!registry.is_empty());
    assert_eq!(registry.len(), 1);
    assert_eq!(registry.cursor(&thread("thread-1")), Some(cursor(0)));
    assert_eq!(
        registry.state(&thread("thread-1")),
        Some(SubscriptionState::Pending)
    );
    let view = registry
        .view(&thread("thread-1"))
        .expect("view should exist");
    assert_eq!(view.lease(), &lease);
    assert_eq!(view.state(), SubscriptionState::Pending);
    assert_eq!(view.cursor(), cursor(0));
}

// ---------------------------------------------------------------------------
// Distinct threads coexist independently
// ---------------------------------------------------------------------------

#[test]
fn distinct_threads_coexist_independently() {
    let mut registry = ConversationSubscriptionRegistry::new();
    let lease_a = registry
        .register(thread("thread-a"), cursor(0))
        .expect("thread a should register");
    let lease_b = registry
        .register(thread("thread-b"), cursor(5))
        .expect("thread b should register");

    assert_ne!(lease_a.generation(), lease_b.generation());
    assert!(lease_b.generation().get() > lease_a.generation().get());
    assert_eq!(registry.len(), 2);
    assert_eq!(registry.cursor(&thread("thread-a")), Some(cursor(0)));
    assert_eq!(registry.cursor(&thread("thread-b")), Some(cursor(5)));

    // Activate only a.
    let returned = registry.activate(&lease_a).expect("activate a");
    assert_eq!(returned, cursor(0));
    assert_eq!(
        registry.state(&thread("thread-a")),
        Some(SubscriptionState::Active)
    );
    assert_eq!(
        registry.state(&thread("thread-b")),
        Some(SubscriptionState::Pending)
    );

    // Publish for a does not affect b.
    let batch_a = batch(thread("thread-a"), 0, 1, vec![append_patch("patch-a-1", 1)]);
    registry
        .apply_batch(&lease_a, &batch_a)
        .expect("batch for a should advance");
    assert_eq!(registry.cursor(&thread("thread-a")), Some(cursor(1)));
    assert_eq!(registry.cursor(&thread("thread-b")), Some(cursor(5)));
}

// ---------------------------------------------------------------------------
// Pending activation
// ---------------------------------------------------------------------------

#[test]
fn pending_activation_returns_exact_cursor_and_second_activation_is_rejected() {
    let mut registry = ConversationSubscriptionRegistry::new();
    let lease = registry
        .register(thread("thread-1"), cursor(7))
        .expect("register should succeed");

    let returned = registry
        .activate(&lease)
        .expect("first activation should succeed");
    assert_eq!(returned, cursor(7));
    assert_eq!(
        registry.state(&thread("thread-1")),
        Some(SubscriptionState::Active)
    );

    let before = registry.clone();
    let err = registry
        .activate(&lease)
        .expect_err("second activation should fail");
    assert_eq!(err, ActivateError::AlreadyActive);
    assert_eq!(registry, before, "second activation must not mutate state");

    // Applying with the same lease still works because it is now active.
    let batch = batch(thread("thread-1"), 7, 8, vec![append_patch("patch-1", 8)]);
    registry
        .apply_batch(&lease, &batch)
        .expect("active lease should accept contiguous batch");
    assert_eq!(registry.cursor(&thread("thread-1")), Some(cursor(8)));
}

// ---------------------------------------------------------------------------
// Replacement of Pending and Active entries
// ---------------------------------------------------------------------------

#[test]
fn replacement_of_pending_entry_mints_newer_lease_and_stales_old_one() {
    let mut registry = ConversationSubscriptionRegistry::new();
    let lease_old = registry
        .register(thread("thread-1"), cursor(0))
        .expect("first pending");
    let lease_new = registry
        .register(thread("thread-1"), cursor(3))
        .expect("replacement pending");

    assert!(lease_new.generation().get() > lease_old.generation().get());
    assert_eq!(registry.cursor(&thread("thread-1")), Some(cursor(3)));
    assert_eq!(
        registry.state(&thread("thread-1")),
        Some(SubscriptionState::Pending)
    );

    let before = registry.clone();
    assert_eq!(
        registry.activate(&lease_old),
        Err(ActivateError::StaleLease)
    );
    assert_eq!(registry, before, "stale activation must not mutate");

    // Old lease cannot publish.
    let batch = batch(thread("thread-1"), 3, 4, vec![append_patch("patch-new", 4)]);
    assert_eq!(
        registry.apply_batch(&lease_old, &batch),
        Err(ApplyBatchError::StaleLease)
    );
    assert_eq!(registry, before);

    // New lease activates and publishes.
    registry
        .activate(&lease_new)
        .expect("new lease should activate");
    registry
        .apply_batch(&lease_new, &batch)
        .expect("new lease should publish");
    assert_eq!(registry.cursor(&thread("thread-1")), Some(cursor(4)));
}

#[test]
fn replacement_of_active_entry_mints_newer_pending_and_stales_old_active_lease() {
    let mut registry = ConversationSubscriptionRegistry::new();
    let lease_active = registry
        .register(thread("thread-1"), cursor(10))
        .expect("initial pending");
    registry.activate(&lease_active).expect("activate");

    let batch1 = batch(
        thread("thread-1"),
        10,
        11,
        vec![append_patch("patch-1", 11)],
    );
    registry
        .apply_batch(&lease_active, &batch1)
        .expect("advance to 11");

    let lease_new = registry
        .register(thread("thread-1"), cursor(99))
        .expect("replacement of active");
    assert!(lease_new.generation().get() > lease_active.generation().get());
    assert_eq!(
        registry.state(&thread("thread-1")),
        Some(SubscriptionState::Pending)
    );
    assert_eq!(registry.cursor(&thread("thread-1")), Some(cursor(99)));

    let before = registry.clone();
    assert_eq!(
        registry.apply_batch(
            &lease_active,
            &batch(
                thread("thread-1"),
                11,
                12,
                vec![append_patch("patch-old", 12)]
            )
        ),
        Err(ApplyBatchError::StaleLease)
    );
    assert_eq!(registry, before);
    assert_eq!(
        registry.activate(&lease_active),
        Err(ActivateError::StaleLease)
    );
    assert_eq!(registry, before);

    // New lease must be activated before publishing.
    assert_eq!(
        registry.apply_batch(
            &lease_new,
            &batch(
                thread("thread-1"),
                99,
                100,
                vec![append_patch("patch-100", 100)]
            )
        ),
        Err(ApplyBatchError::NotActive)
    );
    registry.activate(&lease_new).expect("activate replacement");
    registry
        .apply_batch(
            &lease_new,
            &batch(
                thread("thread-1"),
                99,
                100,
                vec![append_patch("patch-100", 100)],
            ),
        )
        .expect("publish after activation");
    assert_eq!(registry.cursor(&thread("thread-1")), Some(cursor(100)));
}

// ---------------------------------------------------------------------------
// Unsubscribe
// ---------------------------------------------------------------------------

#[test]
fn unsubscribe_pending_and_active_and_unknown_and_removed_lease_rejection() {
    let mut registry = ConversationSubscriptionRegistry::new();
    let lease_pending = registry
        .register(thread("thread-pending"), cursor(1))
        .expect("pending");
    let lease_active = registry
        .register(thread("thread-active"), cursor(2))
        .expect("pending active");
    registry.activate(&lease_active).expect("activate");

    // Unsubscribe pending.
    let outcome = registry.unsubscribe(&thread("thread-pending"));
    match outcome {
        UnsubscribeOutcome::Removed(removed) => {
            assert_eq!(removed.lease(), &lease_pending);
            assert_eq!(removed.state(), SubscriptionState::Pending);
            assert_eq!(removed.cursor(), cursor(1));
        }
        UnsubscribeOutcome::Absent => panic!("pending unsubscribe should be removed"),
    }
    assert!(!registry.contains(&thread("thread-pending")));
    assert_eq!(registry.len(), 1);
    // Removed lease is stale forever.
    assert_eq!(
        registry.activate(&lease_pending),
        Err(ActivateError::StaleLease)
    );
    assert_eq!(
        registry.apply_batch(
            &lease_pending,
            &batch(thread("thread-pending"), 1, 2, vec![append_patch("p", 2)])
        ),
        Err(ApplyBatchError::StaleLease)
    );

    // Unsubscribe active.
    let outcome = registry.unsubscribe(&thread("thread-active"));
    match outcome {
        UnsubscribeOutcome::Removed(removed) => {
            assert_eq!(removed.lease(), &lease_active);
            assert_eq!(removed.state(), SubscriptionState::Active);
            assert_eq!(removed.cursor(), cursor(2));
        }
        UnsubscribeOutcome::Absent => panic!("active unsubscribe should be removed"),
    }
    assert!(registry.is_empty());
    assert_eq!(
        registry.activate(&lease_active),
        Err(ActivateError::StaleLease)
    );

    // Unknown unsubscribe is typed absent/no-op.
    let before = registry.clone();
    assert_eq!(
        registry.unsubscribe(&thread("thread-unknown")),
        UnsubscribeOutcome::Absent
    );
    assert_eq!(registry, before);

    // Unsubscribing again is absent.
    assert_eq!(
        registry.unsubscribe(&thread("thread-pending")),
        UnsubscribeOutcome::Absent
    );
}

// ---------------------------------------------------------------------------
// Matching active PatchBatch advances exactly
// ---------------------------------------------------------------------------

#[test]
fn matching_active_patch_batch_advances_exactly_from_to() {
    let mut registry = ConversationSubscriptionRegistry::new();
    let lease = registry
        .register(thread("thread-1"), cursor(0))
        .expect("register");
    registry.activate(&lease).expect("activate");

    let batch = batch(
        thread("thread-1"),
        0,
        2,
        vec![append_patch("p1", 1), append_patch("p2", 2)],
    );
    let new_cursor = registry
        .apply_batch(&lease, &batch)
        .expect("apply should succeed");
    assert_eq!(new_cursor, cursor(2));
    assert_eq!(registry.cursor(&thread("thread-1")), Some(cursor(2)));
}

// ---------------------------------------------------------------------------
// Pending publication rejection
// ---------------------------------------------------------------------------

#[test]
fn pending_publication_is_rejected_without_mutation() {
    let mut registry = ConversationSubscriptionRegistry::new();
    let lease = registry
        .register(thread("thread-1"), cursor(4))
        .expect("register pending");
    let batch = batch(thread("thread-1"), 4, 5, vec![append_patch("p5", 5)]);
    let before = registry.clone();
    assert_eq!(
        registry.apply_batch(&lease, &batch),
        Err(ApplyBatchError::NotActive)
    );
    assert_eq!(registry, before);
}

// ---------------------------------------------------------------------------
// Wrong-thread, stale, duplicate/regression/gap rejection with preservation
// ---------------------------------------------------------------------------

#[test]
fn wrong_thread_lease_and_batch_are_rejected_with_preservation() {
    let mut registry = ConversationSubscriptionRegistry::new();
    let lease_a = registry.register(thread("thread-a"), cursor(0)).expect("a");
    let lease_b = registry.register(thread("thread-b"), cursor(0)).expect("b");
    registry.activate(&lease_a).expect("activate a");
    registry.activate(&lease_b).expect("activate b");

    let before = registry.clone();
    // Lease for a with batch for b (thread mismatch).
    let batch_b = batch(thread("thread-b"), 0, 1, vec![append_patch("p-b-1", 1)]);
    assert_eq!(
        registry.apply_batch(&lease_a, &batch_b),
        Err(ApplyBatchError::ThreadMismatch)
    );
    assert_eq!(registry, before);

    // Lease for b with batch for a (thread mismatch via lease thread check).
    let batch_a = batch(thread("thread-a"), 0, 1, vec![append_patch("p-a-1", 1)]);
    assert_eq!(
        registry.apply_batch(&lease_b, &batch_a),
        Err(ApplyBatchError::ThreadMismatch)
    );
    assert_eq!(registry, before);
}

#[test]
fn stale_lease_is_rejected_for_activation_and_publication_with_preservation() {
    let mut registry = ConversationSubscriptionRegistry::new();
    let lease_old = registry
        .register(thread("thread-1"), cursor(0))
        .expect("old");
    let lease_new = registry
        .register(thread("thread-1"), cursor(0))
        .expect("new");
    // Only new is current.
    registry.activate(&lease_new).expect("activate new");
    let before = registry.clone();
    assert_eq!(
        registry.activate(&lease_old),
        Err(ActivateError::StaleLease)
    );
    assert_eq!(registry, before);
    let batch = batch(thread("thread-1"), 0, 1, vec![append_patch("p1", 1)]);
    assert_eq!(
        registry.apply_batch(&lease_old, &batch),
        Err(ApplyBatchError::StaleLease)
    );
    assert_eq!(registry, before);
}

#[test]
fn duplicate_regression_and_gap_are_rejected_with_equality_preservation() {
    let mut registry = ConversationSubscriptionRegistry::new();
    let lease = registry
        .register(thread("thread-1"), cursor(10))
        .expect("register");
    registry.activate(&lease).expect("activate");
    // Advance to 12.
    let batch_advance = batch(
        thread("thread-1"),
        10,
        12,
        vec![append_patch("p11", 11), append_patch("p12", 12)],
    );
    registry
        .apply_batch(&lease, &batch_advance)
        .expect("advance");
    assert_eq!(registry.cursor(&thread("thread-1")), Some(cursor(12)));
    let before = registry.clone();

    // Duplicate: re-apply same interval 10->12.
    let duplicate = batch(
        thread("thread-1"),
        10,
        12,
        vec![append_patch("p11-dup", 11), append_patch("p12-dup", 12)],
    );
    assert_eq!(
        registry.apply_batch(&lease, &duplicate),
        Err(ApplyBatchError::CursorMismatch {
            expected: cursor(12),
            actual: cursor(10)
        })
    );
    assert_eq!(registry, before);

    // Regression: from 5 behind current.
    let regression = batch(thread("thread-1"), 5, 6, vec![append_patch("p6", 6)]);
    assert_eq!(
        registry.apply_batch(&lease, &regression),
        Err(ApplyBatchError::CursorMismatch {
            expected: cursor(12),
            actual: cursor(5)
        })
    );
    assert_eq!(registry, before);

    // Gap: skip 13.
    let gap = batch(thread("thread-1"), 13, 14, vec![append_patch("p14", 14)]);
    assert_eq!(
        registry.apply_batch(&lease, &gap),
        Err(ApplyBatchError::CursorMismatch {
            expected: cursor(12),
            actual: cursor(13)
        })
    );
    assert_eq!(registry, before);

    // Exact next is accepted.
    let next = batch(thread("thread-1"), 12, 13, vec![append_patch("p13", 13)]);
    registry
        .apply_batch(&lease, &next)
        .expect("next should succeed");
    assert_eq!(registry.cursor(&thread("thread-1")), Some(cursor(13)));
}

// ---------------------------------------------------------------------------
// Sequential batches advance contiguously and isolation across threads
// ---------------------------------------------------------------------------

#[test]
fn sequential_batches_advance_contiguously_and_never_affect_another_thread() {
    let mut registry = ConversationSubscriptionRegistry::new();
    let lease_a = registry.register(thread("thread-a"), cursor(0)).expect("a");
    let lease_b = registry
        .register(thread("thread-b"), cursor(100))
        .expect("b");
    registry.activate(&lease_a).expect("activate a");
    registry.activate(&lease_b).expect("activate b");

    for seq in 1..=5 {
        let batch_a = batch(
            thread("thread-a"),
            seq - 1,
            seq,
            vec![append_patch(&format!("a-{seq}"), seq)],
        );
        registry.apply_batch(&lease_a, &batch_a).expect("a batch");
        assert_eq!(registry.cursor(&thread("thread-a")), Some(cursor(seq)));
        assert_eq!(
            registry.cursor(&thread("thread-b")),
            Some(cursor(100)),
            "b must not move while a advances"
        );
    }

    for seq in 101..=103 {
        let batch_b = batch(
            thread("thread-b"),
            seq - 1,
            seq,
            vec![append_patch(&format!("b-{seq}"), seq)],
        );
        registry.apply_batch(&lease_b, &batch_b).expect("b batch");
        assert_eq!(registry.cursor(&thread("thread-b")), Some(cursor(seq)));
        assert_eq!(
            registry.cursor(&thread("thread-a")),
            Some(cursor(5)),
            "a must not move while b advances"
        );
    }

    // Interleaved check: a's next must still be 5->6, not affected by b.
    let next_a = batch(thread("thread-a"), 5, 6, vec![append_patch("a-6", 6)]);
    registry.apply_batch(&lease_a, &next_a).expect("a next");
    assert_eq!(registry.cursor(&thread("thread-a")), Some(cursor(6)));
}

// ---------------------------------------------------------------------------
// Error display does not leak patch bodies
// ---------------------------------------------------------------------------

#[test]
fn error_display_does_not_include_patch_bodies() {
    let lease = ConversationSubscriptionRegistry::new()
        .register(thread("thread-1"), cursor(0))
        .expect("register")
        .clone();
    // Exercise display of errors without constructing a patch body payload.
    let stale_display = format!("{}", ApplyBatchError::StaleLease);
    assert!(!stale_display.contains("patch-body-sentinel"));
    let mismatch_display = format!(
        "{}",
        ApplyBatchError::CursorMismatch {
            expected: cursor(1),
            actual: cursor(2)
        }
    );
    assert!(!mismatch_display.contains("patch-body-sentinel"));
    let register_display = format!("{}", RegisterError::GenerationExhausted);
    assert!(!register_display.contains("patch-body-sentinel"));
    let activate_display = format!("{}", ActivateError::StaleLease);
    assert!(!activate_display.contains("secret"));
    // Ensure lease debug does not contain patch fragments either.
    let lease_debug = format!("{lease:?}");
    assert!(!lease_debug.contains("patch-body-sentinel"));
}

// ---------------------------------------------------------------------------
// Equality and clone semantics
// ---------------------------------------------------------------------------

#[test]
fn registry_equality_is_deterministic_and_clone_preserves_state() {
    let mut registry = ConversationSubscriptionRegistry::new();
    let lease = registry
        .register(thread("thread-1"), cursor(3))
        .expect("register");
    registry.activate(&lease).expect("activate");
    let cloned = registry.clone();
    assert_eq!(registry, cloned);
    let batch = batch(thread("thread-1"), 3, 4, vec![append_patch("p4", 4)]);
    let mut mutated = cloned.clone();
    mutated.apply_batch(&lease, &batch).expect("apply");
    assert_ne!(registry, mutated);
    assert_eq!(mutated.cursor(&thread("thread-1")), Some(cursor(4)));
    assert_eq!(registry.cursor(&thread("thread-1")), Some(cursor(3)));
}

#[test]
fn lease_is_cloneable_and_equality_testable_and_carries_thread_and_generation() {
    let mut registry = ConversationSubscriptionRegistry::new();
    let lease = registry
        .register(thread("thread-x"), cursor(0))
        .expect("register");
    let lease_clone = lease.clone();
    assert_eq!(lease, lease_clone);
    assert_eq!(lease.thread_id(), &thread("thread-x"));
    assert_eq!(lease.generation(), lease_clone.generation());
}

#[test]
fn lease_accessor_and_publish_alias_are_exercised() {
    let mut registry = ConversationSubscriptionRegistry::new();
    let lease = registry
        .register(thread("thread-alias"), cursor(0))
        .expect("register");
    // Exercise the `lease()` accessor.
    let fetched = registry
        .lease(&thread("thread-alias"))
        .expect("lease accessor should return current lease");
    assert_eq!(fetched, lease);
    assert_eq!(fetched.thread_id(), &thread("thread-alias"));

    registry.activate(&lease).expect("activate");
    let batch = batch(
        thread("thread-alias"),
        0,
        1,
        vec![append_patch("alias-1", 1)],
    );
    // Exercise the `publish_batch` alias (same semantics as `apply_batch`).
    let advanced = registry
        .publish_batch(&lease, &batch)
        .expect("publish alias should advance");
    assert_eq!(advanced, cursor(1));
    assert_eq!(registry.cursor(&thread("thread-alias")), Some(cursor(1)));
}
