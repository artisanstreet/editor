//! Conversation snapshot, cursor, and replay-boundary coverage.

use artisan_domain::{
    CONVERSATION_PATCH_BATCH_MAX_PATCHES, CONVERSATION_QUERY_MAX_TURNS,
    CONVERSATION_TEXT_FRAGMENT_MAX_BYTES, ConversationCursor, ConversationItem,
    ConversationLifecycle, ConversationPatch, ConversationQuery, ConversationQueryBounds,
    ConversationRequest, ConversationSnapshot, ConversationSnapshotError, ConversationSubscribe,
    ConversationSubscriptionStart, ConversationUnsubscribe, CounterError, IncrementalText,
    IncrementalTextError, ItemId, ItemOrdinal, MessageBody, PatchBatch, PatchBatchError, PatchId,
    PatchSequence, QueryTurnCount, QueryTurnCountError, Revision, ThreadId, TurnId, TurnOrdinal,
    UnixMillis, UserMessageItem,
};

fn turn(id: &str, ordinal: u64) -> artisan_domain::ConversationTurn {
    artisan_domain::ConversationTurn {
        turn_id: TurnId::parse(id).expect("fixture turn id is valid"),
        ordinal: TurnOrdinal::new(ordinal),
        revision: Revision::default(),
        lifecycle: ConversationLifecycle::Pending,
        created_at: UnixMillis::from_millis(-10),
        updated_at: UnixMillis::from_millis(20),
    }
}

fn item(id: &str, turn_id: &str, ordinal: u64) -> ConversationItem {
    ConversationItem::UserMessage(UserMessageItem {
        item_id: ItemId::parse(id).expect("fixture item id is valid"),
        turn_id: TurnId::parse(turn_id).expect("fixture turn id is valid"),
        ordinal: ItemOrdinal::new(ordinal),
        revision: Revision::default(),
        lifecycle: ConversationLifecycle::Pending,
        body: MessageBody::parse("Queued text").expect("fixture body is valid"),
        created_at: UnixMillis::from_millis(-10),
        updated_at: UnixMillis::from_millis(20),
    })
}

fn append_patch(id: impl Into<String>, sequence: u64) -> ConversationPatch {
    ConversationPatch::ItemAppend {
        patch_id: PatchId::parse(id.into()).expect("fixture patch id is valid"),
        sequence: PatchSequence::new(sequence).expect("fixture sequence is positive"),
        item_id: ItemId::parse("item-1").expect("fixture item id is valid"),
        revision: Revision::new(sequence),
        text: IncrementalText::parse("delta").expect("fixture fragment is valid"),
    }
}

fn thread_id() -> ThreadId {
    ThreadId::parse("thread-1").expect("fixture thread id is valid")
}

#[test]
fn zero_based_counters_and_one_based_patch_sequences_are_explicit() {
    assert_eq!(ConversationCursor::default().get(), 0);
    assert_eq!(TurnOrdinal::default().get(), 0);
    assert_eq!(ItemOrdinal::default().get(), 0);
    assert_eq!(Revision::default().get(), 0);
    assert_eq!(PatchSequence::new(0), Err(CounterError::ZeroPatchSequence));
    assert_eq!(
        PatchSequence::new(1)
            .expect("one is the first sequence")
            .get(),
        1
    );

    assert_eq!(
        Revision::new(u64::MAX).checked_next(),
        Err(CounterError::Overflow {
            counter: "revision",
            value: u64::MAX,
        })
    );
    assert_eq!(
        ConversationCursor::new(u64::MAX).checked_next_sequence(),
        Err(CounterError::Overflow {
            counter: "conversation cursor",
            value: u64::MAX,
        })
    );
}

#[test]
fn incremental_text_uses_utf8_bytes_and_permits_an_empty_opening_fragment() {
    assert_eq!(
        IncrementalText::parse("")
            .expect("an empty opening fragment is valid")
            .as_str(),
        ""
    );
    assert!(IncrementalText::parse("a".repeat(CONVERSATION_TEXT_FRAGMENT_MAX_BYTES)).is_ok());

    let multibyte = "🦀".repeat(CONVERSATION_TEXT_FRAGMENT_MAX_BYTES / 4 + 1);
    assert_eq!(
        IncrementalText::parse(multibyte.clone()),
        Err(IncrementalTextError::TooLong {
            length: multibyte.len(),
            maximum: CONVERSATION_TEXT_FRAGMENT_MAX_BYTES,
        })
    );
}

#[test]
fn snapshot_validates_ids_ordinals_and_turn_ownership() {
    let snapshot = ConversationSnapshot::new(
        thread_id(),
        ConversationCursor::new(7),
        vec![turn("turn-1", 0)],
        vec![item("item-1", "turn-1", 1)],
        UnixMillis::from_millis(-1),
    )
    .expect("valid canonical snapshot");
    assert_eq!(snapshot.cursor().get(), 7);
    assert_eq!(snapshot.turns().len(), 1);
    assert_eq!(snapshot.items().len(), 1);
    assert_eq!(snapshot.updated_at(), UnixMillis::from_millis(-1));

    assert_eq!(
        ConversationSnapshot::new(
            thread_id(),
            ConversationCursor::default(),
            vec![turn("turn-1", 0), turn("turn-1", 1)],
            Vec::new(),
            UnixMillis::from_millis(0),
        ),
        Err(ConversationSnapshotError::DuplicateTurnId {
            turn_id: TurnId::parse("turn-1").expect("fixture is valid"),
        })
    );

    assert_eq!(
        ConversationSnapshot::new(
            thread_id(),
            ConversationCursor::default(),
            vec![turn("turn-1", 0)],
            vec![item("item-1", "turn-1", 0)],
            UnixMillis::from_millis(0),
        ),
        Err(ConversationSnapshotError::DuplicateOrdinal { ordinal: 0 })
    );

    assert_eq!(
        ConversationSnapshot::new(
            thread_id(),
            ConversationCursor::default(),
            vec![turn("turn-1", 0)],
            vec![item("item-1", "turn-missing", 1)],
            UnixMillis::from_millis(0),
        ),
        Err(ConversationSnapshotError::UnknownTurn {
            item_id: ItemId::parse("item-1").expect("fixture is valid"),
            turn_id: TurnId::parse("turn-missing").expect("fixture is valid"),
        })
    );
}

#[test]
fn snapshot_canonicalizes_out_of_order_input_by_shared_ordinals() {
    let snapshot = ConversationSnapshot::new(
        thread_id(),
        ConversationCursor::default(),
        vec![turn("turn-later", 4), turn("turn-first", 0)],
        vec![
            item("item-later", "turn-later", 3),
            item("item-first", "turn-first", 1),
        ],
        UnixMillis::from_millis(0),
    )
    .expect("out-of-order input is canonicalized, not rejected");

    assert_eq!(
        snapshot
            .turns()
            .iter()
            .map(|turn| turn.ordinal.get())
            .collect::<Vec<_>>(),
        vec![0, 4]
    );
    assert_eq!(
        snapshot
            .items()
            .iter()
            .map(ConversationItem::ordinal)
            .map(ItemOrdinal::get)
            .collect::<Vec<_>>(),
        vec![1, 3]
    );
}

#[test]
fn fresh_subscription_has_a_distinct_snapshot_first_value() {
    let snapshot = ConversationSnapshot::new(
        thread_id(),
        ConversationCursor::default(),
        Vec::new(),
        Vec::new(),
        UnixMillis::from_millis(0),
    )
    .expect("empty initial snapshot is valid");
    let subscribe = ConversationSubscribe::fresh(thread_id());
    assert_eq!(subscribe.after, None);

    let start = ConversationSubscriptionStart::new(snapshot);
    assert_eq!(start.snapshot().thread_id(), &subscribe.thread_id);

    let resumed = ConversationSubscribe::resume(thread_id(), ConversationCursor::new(9));
    assert_eq!(resumed.after, Some(ConversationCursor::new(9)));

    assert!(matches!(
        ConversationRequest::Subscribe(subscribe),
        ConversationRequest::Subscribe(_)
    ));
    assert!(matches!(
        ConversationRequest::Unsubscribe(ConversationUnsubscribe {
            thread_id: thread_id()
        }),
        ConversationRequest::Unsubscribe(_)
    ));
}

#[test]
fn patch_batch_accepts_exactly_one_bounded_contiguous_interval() {
    let batch = PatchBatch::new(
        thread_id(),
        ConversationCursor::default(),
        ConversationCursor::new(2),
        vec![append_patch("patch-1", 1), append_patch("patch-2", 2)],
    )
    .expect("sequences one and two are contiguous after cursor zero");
    assert_eq!(batch.from_cursor(), ConversationCursor::default());
    assert_eq!(batch.to_cursor(), ConversationCursor::new(2));
    assert_eq!(batch.patches().len(), 2);
    assert_eq!(batch.thread_id(), &thread_id());

    let maximum = (1..=CONVERSATION_PATCH_BATCH_MAX_PATCHES)
        .map(|index| {
            let sequence = u64::try_from(index).expect("fixture size fits u64");
            append_patch(format!("patch-{sequence}"), sequence)
        })
        .collect();
    assert!(
        PatchBatch::new(
            thread_id(),
            ConversationCursor::default(),
            ConversationCursor::new(
                u64::try_from(CONVERSATION_PATCH_BATCH_MAX_PATCHES)
                    .expect("batch ceiling fits u64"),
            ),
            maximum,
        )
        .is_ok()
    );
}

#[test]
fn patch_batch_reports_each_replay_boundary_failure() {
    assert_eq!(
        PatchBatch::new(
            thread_id(),
            ConversationCursor::default(),
            ConversationCursor::default(),
            Vec::new(),
        ),
        Err(PatchBatchError::Empty)
    );

    let over_bound = (1..=CONVERSATION_PATCH_BATCH_MAX_PATCHES + 1)
        .map(|index| {
            let sequence = u64::try_from(index).expect("fixture size fits u64");
            append_patch(format!("patch-{sequence}"), sequence)
        })
        .collect();
    assert_eq!(
        PatchBatch::new(
            thread_id(),
            ConversationCursor::default(),
            ConversationCursor::new(65),
            over_bound,
        ),
        Err(PatchBatchError::TooManyPatches {
            count: CONVERSATION_PATCH_BATCH_MAX_PATCHES + 1,
            maximum: CONVERSATION_PATCH_BATCH_MAX_PATCHES,
        })
    );

    assert_eq!(
        PatchBatch::new(
            thread_id(),
            ConversationCursor::default(),
            ConversationCursor::new(2),
            vec![append_patch("same-patch", 1), append_patch("same-patch", 2)],
        ),
        Err(PatchBatchError::DuplicatePatchId {
            patch_id: PatchId::parse("same-patch").expect("fixture is valid"),
        })
    );

    assert_eq!(
        PatchBatch::new(
            thread_id(),
            ConversationCursor::default(),
            ConversationCursor::new(1),
            vec![append_patch("patch-1", 1), append_patch("patch-copy", 1)],
        ),
        Err(PatchBatchError::DuplicateSequence { sequence: 1 })
    );

    assert_eq!(
        PatchBatch::new(
            thread_id(),
            ConversationCursor::new(2),
            ConversationCursor::new(1),
            vec![append_patch("old-patch", 1)],
        ),
        Err(PatchBatchError::OutOfOrder {
            previous: 2,
            actual: 1,
        })
    );

    assert_eq!(
        PatchBatch::new(
            thread_id(),
            ConversationCursor::default(),
            ConversationCursor::new(2),
            vec![append_patch("patch-2", 2)],
        ),
        Err(PatchBatchError::Gap {
            expected: 1,
            actual: 2,
        })
    );

    assert_eq!(
        PatchBatch::new(
            thread_id(),
            ConversationCursor::default(),
            ConversationCursor::new(9),
            vec![append_patch("patch-1", 1)],
        ),
        Err(PatchBatchError::EndpointMismatch {
            declared: 9,
            actual: 1,
        })
    );

    assert_eq!(
        PatchBatch::new(
            thread_id(),
            ConversationCursor::new(u64::MAX),
            ConversationCursor::new(u64::MAX),
            vec![append_patch("patch-max", u64::MAX)],
        ),
        Err(PatchBatchError::Counter(CounterError::Overflow {
            counter: "conversation cursor",
            value: u64::MAX,
        }))
    );
}

#[test]
fn bounded_query_turn_count_preserves_legacy_edges() {
    assert_eq!(QueryTurnCount::new(0), Err(QueryTurnCountError::Zero));
    assert_eq!(
        QueryTurnCount::new(u64::from(CONVERSATION_QUERY_MAX_TURNS))
            .expect("the documented maximum is valid")
            .get(),
        CONVERSATION_QUERY_MAX_TURNS
    );
    assert_eq!(
        QueryTurnCount::new(u64::from(CONVERSATION_QUERY_MAX_TURNS) + 1),
        Err(QueryTurnCountError::TooLarge {
            value: u64::from(CONVERSATION_QUERY_MAX_TURNS) + 1,
            maximum: CONVERSATION_QUERY_MAX_TURNS,
        })
    );

    let query = ConversationQuery {
        thread_id: thread_id(),
        bounds: ConversationQueryBounds::Window {
            maximum_turn_count: QueryTurnCount::new(128).expect("legacy tail window is bounded"),
        },
    };
    assert!(matches!(
        ConversationRequest::Query(query),
        ConversationRequest::Query(_)
    ));
}
