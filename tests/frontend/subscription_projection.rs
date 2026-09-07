//! Deterministic coverage for the thread-isolated conversation subscription
//! delivery state machine.

use artisan_domain::{
    ConversationCursor, ConversationItem, ConversationLifecycle, ConversationPatch,
    ConversationSnapshot, ConversationSubscriptionStart, ConversationTurn, IncrementalText, ItemId,
    ItemOrdinal, MessageBody, PatchBatch, PatchId, PatchSequence, Revision, ThreadId, TurnId,
    TurnOrdinal, UnixMillis, UserMessageItem,
};
use artisan_frontend::conversation_projection::{
    ConversationProjection, ProjectionError, ProjectionStatus,
};
use artisan_frontend::subscription_projection::{
    ActivationOutcome, DeliveryLostReason, DeliveryOutcome, MAX_PENDING_BATCHES,
    SubscriptionProjectionError, SubscriptionProjectionRegistry, SubscriptionStatus,
    UnsubscribeOutcome,
};
use artisan_protocol::{ConversationSubscriptionStarted, ConversationSubscriptionStopped};

const THREAD_ONE: &str = "subscription-thread-one";
const THREAD_TWO: &str = "subscription-thread-two";

fn thread_id(value: &str) -> ThreadId {
    ThreadId::parse(value).expect("fixture thread id is valid")
}

fn stamp(value: i64) -> UnixMillis {
    UnixMillis::from_millis(value)
}

fn conversation_snapshot(
    thread: &ThreadId,
    cursor: u64,
    body: &str,
    item_revision: u64,
    item_updated_at: i64,
    watermark: i64,
) -> ConversationSnapshot {
    let turn_id = TurnId::parse(format!("turn-{}", thread.as_str())).expect("turn id is valid");
    let item_id = ItemId::parse(format!("item-{}", thread.as_str())).expect("item id is valid");
    ConversationSnapshot::new(
        thread.clone(),
        ConversationCursor::new(cursor),
        vec![ConversationTurn {
            turn_id: turn_id.clone(),
            ordinal: TurnOrdinal::new(0),
            revision: Revision::new(0),
            lifecycle: ConversationLifecycle::Pending,
            created_at: stamp(-10),
            updated_at: stamp(20),
        }],
        vec![ConversationItem::UserMessage(UserMessageItem {
            item_id,
            turn_id,
            ordinal: ItemOrdinal::new(1),
            revision: Revision::new(item_revision),
            lifecycle: ConversationLifecycle::Pending,
            body: MessageBody::parse(body.to_owned()).expect("fixture body is valid"),
            created_at: stamp(-5),
            updated_at: stamp(item_updated_at),
        })],
        stamp(watermark),
    )
    .expect("fixture snapshot is structurally valid")
}

fn ready_projection(thread: &ThreadId, cursor: u64, body: &str) -> ConversationProjection {
    let mut projection = ConversationProjection::new(thread.clone());
    projection
        .install_snapshot(&conversation_snapshot(thread, cursor, body, 0, 25, 30))
        .expect("fixture baseline installs");
    projection
}

fn append_batch(thread: &ThreadId, from: u64, to: u64, revision: u64, text: &str) -> PatchBatch {
    let item_id = ItemId::parse(format!("item-{}", thread.as_str())).expect("item id is valid");
    let patch_id = PatchId::parse(format!("patch-{from}-{to}-{revision}-{}", text.len()))
        .expect("patch id is valid");
    PatchBatch::new(
        thread.clone(),
        ConversationCursor::new(from),
        ConversationCursor::new(to),
        vec![ConversationPatch::ItemAppend {
            patch_id,
            sequence: PatchSequence::new(to).expect("patch sequence is positive"),
            item_id,
            revision: Revision::new(revision),
            text: IncrementalText::parse(text.to_owned()).expect("fragment is valid"),
            updated_at: stamp(31 + i64::try_from(to).expect("fixture cursor fits")),
        }],
    )
    .expect("fixture batch is structurally contiguous")
}

fn fresh_start(thread: &ThreadId, cursor: u64, body: &str) -> ConversationSubscriptionStarted {
    ConversationSubscriptionStarted::Fresh(ConversationSubscriptionStart::new(
        conversation_snapshot(thread, cursor, body, 0, 25, 30),
    ))
}

fn visible_snapshot(
    registry: &SubscriptionProjectionRegistry,
    handle: &artisan_frontend::subscription_projection::SubscriptionHandle,
) -> ConversationSnapshot {
    registry
        .with_projection(handle, |projection| projection.snapshot().cloned())
        .flatten()
        .expect("visible snapshot exists")
}

fn assert_send_sync<T: Send + Sync>() {}

#[test]
fn fresh_activation_drains_contiguous_early_batches_in_arrival_order() {
    let thread = thread_id(THREAD_ONE);
    let registry = SubscriptionProjectionRegistry::new();
    let handle = registry
        .register(thread.clone())
        .expect("registration succeeds");

    assert_eq!(registry.status(&handle), Some(SubscriptionStatus::Pending));
    assert_eq!(
        registry.deliver(&handle, append_batch(&thread, 0, 1, 1, "one")),
        Ok(DeliveryOutcome::Queued { queued_batches: 1 })
    );
    assert_eq!(
        registry.deliver(&handle, append_batch(&thread, 1, 2, 2, " two")),
        Ok(DeliveryOutcome::Queued { queued_batches: 2 })
    );

    let outcome = registry
        .start(&handle, &fresh_start(&thread, 0, "seed"))
        .expect("fresh activation succeeds");
    assert_eq!(
        outcome,
        ActivationOutcome::Fresh {
            cursor: ConversationCursor::new(2),
            drained_batches: 2,
        }
    );
    assert_eq!(registry.status(&handle), Some(SubscriptionStatus::Active));
    assert_eq!(registry.cursor(&handle), Some(ConversationCursor::new(2)));

    let visible = visible_snapshot(&registry, &handle);
    assert_eq!(visible.cursor(), ConversationCursor::new(2));
    let ConversationItem::UserMessage(message) = &visible.items()[0] else {
        panic!("user message fixture is present");
    };
    assert_eq!(message.body.as_str(), "seedone two");
}

#[test]
fn resumed_activation_requires_matching_ready_baseline_and_cursor() {
    let thread = thread_id(THREAD_ONE);
    let registry = SubscriptionProjectionRegistry::new();
    let handle = registry
        .register_with_projection(ready_projection(&thread, 4, "seed"))
        .expect("registration succeeds");

    assert_eq!(
        registry.deliver(&handle, append_batch(&thread, 4, 5, 1, " next")),
        Ok(DeliveryOutcome::Queued { queued_batches: 1 })
    );
    let outcome = registry
        .start(
            &handle,
            &ConversationSubscriptionStarted::Resumed {
                thread_id: thread.clone(),
                cursor: ConversationCursor::new(4),
            },
        )
        .expect("matching resume succeeds");
    assert_eq!(
        outcome,
        ActivationOutcome::Resumed {
            cursor: ConversationCursor::new(5),
            drained_batches: 1,
        }
    );
    assert_eq!(registry.status(&handle), Some(SubscriptionStatus::Active));

    let mismatch_registry = SubscriptionProjectionRegistry::new();
    let mismatch_handle = mismatch_registry
        .register_with_projection(ready_projection(&thread, 4, "seed"))
        .expect("registration succeeds");
    let error = mismatch_registry
        .start(
            &mismatch_handle,
            &ConversationSubscriptionStarted::Resumed {
                thread_id: thread.clone(),
                cursor: ConversationCursor::new(3),
            },
        )
        .expect_err("mismatched resume must fail closed");
    assert!(matches!(
        error,
        SubscriptionProjectionError::DeliveryLost {
            reason: DeliveryLostReason::ResumeBaselineMismatch {
                projection_status: ProjectionStatus::Ready,
                ..
            },
            ..
        }
    ));
    assert_eq!(
        mismatch_registry.status(&mismatch_handle),
        Some(SubscriptionStatus::ResnapshotRequired)
    );
}

#[test]
fn pending_queue_accepts_64_batches_then_enters_recovery_without_retaining_the_65th() {
    let thread = thread_id(THREAD_ONE);
    let registry = SubscriptionProjectionRegistry::new();
    let handle = registry
        .register(thread.clone())
        .expect("registration succeeds");

    for index in 0..MAX_PENDING_BATCHES {
        let from = u64::try_from(index).expect("fixture index fits");
        let to = from + 1;
        assert_eq!(
            registry.deliver(&handle, append_batch(&thread, from, to, 1, "x")),
            Ok(DeliveryOutcome::Queued {
                queued_batches: index + 1,
            })
        );
    }

    let overflow_error = registry
        .deliver(
            &handle,
            append_batch(
                &thread,
                u64::try_from(MAX_PENDING_BATCHES).expect("limit fits"),
                u64::try_from(MAX_PENDING_BATCHES + 1).expect("limit fits"),
                1,
                "overflow",
            ),
        )
        .expect_err("65th pending batch must be rejected");
    assert!(matches!(
        overflow_error,
        SubscriptionProjectionError::DeliveryLost {
            reason: DeliveryLostReason::QueueOverflow {
                limit: MAX_PENDING_BATCHES
            },
            ..
        }
    ));
    assert_eq!(
        registry.status(&handle),
        Some(SubscriptionStatus::ResnapshotRequired)
    );

    let outcome = registry
        .start(&handle, &fresh_start(&thread, 64, "authoritative"))
        .expect("fresh recovery succeeds");
    assert_eq!(
        outcome,
        ActivationOutcome::Fresh {
            cursor: ConversationCursor::new(64),
            drained_batches: 0,
        }
    );
    assert_eq!(registry.status(&handle), Some(SubscriptionStatus::Active));
}

#[test]
fn gap_and_duplicate_regression_enter_recovery_preserving_last_good_projection() {
    let thread = thread_id(THREAD_ONE);
    let registry = SubscriptionProjectionRegistry::new();
    let handle = registry
        .register_with_projection(ready_projection(&thread, 0, "stable"))
        .expect("registration succeeds");
    registry
        .start(
            &handle,
            &ConversationSubscriptionStarted::Resumed {
                thread_id: thread.clone(),
                cursor: ConversationCursor::new(0),
            },
        )
        .expect("matching resume activates");
    let before_gap = visible_snapshot(&registry, &handle);

    let gap_error = registry
        .deliver(&handle, append_batch(&thread, 1, 2, 1, "gap"))
        .expect_err("forward gap must be rejected");
    assert!(matches!(
        gap_error,
        SubscriptionProjectionError::DeliveryLost {
            reason: DeliveryLostReason::Projection(ProjectionError::CursorMismatch),
            ..
        }
    ));
    assert_eq!(
        registry.status(&handle),
        Some(SubscriptionStatus::ResnapshotRequired)
    );
    assert_eq!(visible_snapshot(&registry, &handle), before_gap);

    let duplicate_registry = SubscriptionProjectionRegistry::new();
    let duplicate_handle = duplicate_registry
        .register_with_projection(ready_projection(&thread, 0, "stable"))
        .expect("registration succeeds");
    duplicate_registry
        .start(
            &duplicate_handle,
            &ConversationSubscriptionStarted::Resumed {
                thread_id: thread.clone(),
                cursor: ConversationCursor::new(0),
            },
        )
        .expect("matching resume activates");
    let first_batch = append_batch(&thread, 0, 1, 1, " once");
    assert_eq!(
        duplicate_registry.deliver(&duplicate_handle, first_batch.clone()),
        Ok(DeliveryOutcome::Applied {
            to_cursor: ConversationCursor::new(1)
        })
    );
    let after_first = visible_snapshot(&duplicate_registry, &duplicate_handle);
    let duplicate_error = duplicate_registry
        .deliver(&duplicate_handle, first_batch)
        .expect_err("duplicate/regression must be rejected");
    assert!(matches!(
        duplicate_error,
        SubscriptionProjectionError::DeliveryLost {
            reason: DeliveryLostReason::Projection(ProjectionError::CursorMismatch),
            ..
        }
    ));
    assert_eq!(
        duplicate_registry.status(&duplicate_handle),
        Some(SubscriptionStatus::ResnapshotRequired)
    );
    assert_eq!(
        visible_snapshot(&duplicate_registry, &duplicate_handle),
        after_first
    );
}

#[test]
fn unsubscribe_ignores_late_frames_and_new_registration_starts_a_new_generation() {
    let thread = thread_id(THREAD_ONE);
    let registry = SubscriptionProjectionRegistry::new();
    let old_handle = registry
        .register(thread.clone())
        .expect("registration succeeds");
    let stopped = ConversationSubscriptionStopped {
        thread_id: thread.clone(),
    };

    assert_eq!(
        registry.apply_stopped(&old_handle, &stopped),
        Ok(UnsubscribeOutcome::Unsubscribed)
    );
    assert_eq!(
        registry.status(&old_handle),
        Some(SubscriptionStatus::Unsubscribed)
    );

    let late_start = fresh_start(&thread, 0, "late");
    assert_eq!(
        registry.start(&old_handle, &late_start),
        Ok(ActivationOutcome::Ignored)
    );
    assert_eq!(
        registry.deliver(&old_handle, append_batch(&thread, 0, 1, 1, "late")),
        Ok(DeliveryOutcome::Ignored)
    );
    assert_eq!(
        registry.status(&old_handle),
        Some(SubscriptionStatus::Unsubscribed)
    );

    let new_handle = registry
        .register(thread.clone())
        .expect("new registration succeeds");
    assert_ne!(old_handle, new_handle);
    assert_eq!(registry.status(&old_handle), None);
    assert_eq!(
        registry.start(&old_handle, &late_start),
        Ok(ActivationOutcome::Ignored)
    );
    assert_eq!(
        registry.deliver(&old_handle, append_batch(&thread, 0, 1, 1, "late")),
        Ok(DeliveryOutcome::Ignored)
    );

    assert_eq!(
        registry.start(&new_handle, &fresh_start(&thread, 0, "new")),
        Ok(ActivationOutcome::Fresh {
            cursor: ConversationCursor::new(0),
            drained_batches: 0,
        })
    );
    assert_eq!(
        registry.status(&new_handle),
        Some(SubscriptionStatus::Active)
    );
}

#[test]
fn threads_are_isolated_when_one_generation_loses_delivery() {
    let first_thread = thread_id(THREAD_ONE);
    let second_thread = thread_id(THREAD_TWO);
    let registry = SubscriptionProjectionRegistry::new();
    let first_handle = registry
        .register(first_thread.clone())
        .expect("registration succeeds");
    let second_handle = registry
        .register(second_thread.clone())
        .expect("registration succeeds");

    assert!(matches!(
        registry.start(&first_handle, &fresh_start(&first_thread, 0, "first")),
        Ok(ActivationOutcome::Fresh { .. })
    ));
    assert!(matches!(
        registry.start(&second_handle, &fresh_start(&second_thread, 0, "second")),
        Ok(ActivationOutcome::Fresh { .. })
    ));

    let first_error = registry
        .deliver(&first_handle, append_batch(&first_thread, 1, 2, 1, "gap"))
        .expect_err("first thread gap must fail");
    assert!(matches!(
        first_error,
        SubscriptionProjectionError::DeliveryLost { .. }
    ));
    assert_eq!(
        registry.status(&first_handle),
        Some(SubscriptionStatus::ResnapshotRequired)
    );
    assert_eq!(
        registry.status(&second_handle),
        Some(SubscriptionStatus::Active)
    );

    assert_eq!(
        registry.deliver(
            &second_handle,
            append_batch(&second_thread, 0, 1, 1, " continues"),
        ),
        Ok(DeliveryOutcome::Applied {
            to_cursor: ConversationCursor::new(1)
        })
    );
    assert_eq!(
        registry.cursor(&second_handle),
        Some(ConversationCursor::new(1))
    );
}

#[test]
fn fresh_authoritative_snapshot_is_the_explicit_recovery_path() {
    let thread = thread_id(THREAD_ONE);
    let registry = SubscriptionProjectionRegistry::new();
    let handle = registry
        .register_with_projection(ready_projection(&thread, 0, "before"))
        .expect("registration succeeds");
    let resumed = registry
        .start(
            &handle,
            &ConversationSubscriptionStarted::Resumed {
                thread_id: thread.clone(),
                cursor: ConversationCursor::new(0),
            },
        )
        .expect("resume succeeds");
    assert_eq!(
        resumed,
        ActivationOutcome::Resumed {
            cursor: ConversationCursor::new(0),
            drained_batches: 0,
        }
    );

    registry
        .deliver(&handle, append_batch(&thread, 1, 2, 1, "lost"))
        .expect_err("gap must require recovery");
    let preserved = visible_snapshot(&registry, &handle);
    assert_eq!(preserved.cursor(), ConversationCursor::new(0));
    let ConversationItem::UserMessage(message) = &preserved.items()[0] else {
        panic!("user message fixture is present");
    };
    assert_eq!(message.body.as_str(), "before");

    let recovery = ConversationSubscriptionStarted::Fresh(ConversationSubscriptionStart::new(
        conversation_snapshot(&thread, 2, "recovered", 2, 40, 40),
    ));
    assert_eq!(
        registry.start(&handle, &recovery),
        Ok(ActivationOutcome::Fresh {
            cursor: ConversationCursor::new(2),
            drained_batches: 0,
        })
    );
    assert_eq!(registry.status(&handle), Some(SubscriptionStatus::Active));
    let visible = visible_snapshot(&registry, &handle);
    assert_eq!(visible.cursor(), ConversationCursor::new(2));
    let ConversationItem::UserMessage(message) = &visible.items()[0] else {
        panic!("user message fixture is present");
    };
    assert_eq!(message.body.as_str(), "recovered");
}

#[test]
fn result_error_and_status_debug_values_never_contain_message_body_text() {
    const BODY: &str = "private-conversation-body-marker";
    let thread = thread_id(THREAD_ONE);
    let registry = SubscriptionProjectionRegistry::new();
    let handle = registry
        .register_with_projection(ready_projection(&thread, 0, "baseline"))
        .expect("registration succeeds");
    registry
        .start(
            &handle,
            &ConversationSubscriptionStarted::Resumed {
                thread_id: thread.clone(),
                cursor: ConversationCursor::new(0),
            },
        )
        .expect("resume succeeds");

    let outcome = registry
        .deliver(&handle, append_batch(&thread, 1, 2, 1, BODY))
        .expect_err("gap produces payload-free error");
    assert!(!format!("{outcome:?}").contains(BODY));
    assert!(!format!("{outcome}").contains(BODY));

    let status = registry.status(&handle).expect("state remains observable");
    assert!(!format!("{status:?}").contains(BODY));
    assert!(!format!("{:?}", registry.cursor(&handle)).contains(BODY));
}

#[test]
fn registry_is_send_and_sync_for_off_thread_delivery() {
    assert_send_sync::<SubscriptionProjectionRegistry>();
}
