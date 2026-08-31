//! Behavioral coverage for the bounded native conversation projection:
//! atomic snapshot installation/replacement, transactional batch replay,
//! revision/identity/lifecycle/time rules, retention budgets, and recovery.

use artisan_domain::{
    AssistantBody, AssistantMessageItem, AssistantMessagePhase, ConversationCursor,
    ConversationItem, ConversationLifecycle, ConversationPatch, ConversationSnapshot,
    ConversationTurn, IncrementalText, ItemId, ItemOrdinal, MESSAGE_BODY_MAX_BYTES, MessageBody,
    PatchBatch, PatchId, PatchSequence, Revision, RunId, ThreadId, TurnId, TurnOrdinal, UnixMillis,
    UserMessageItem,
};
use artisan_frontend::conversation_projection::{
    ConversationProjection, ProjectionError, ProjectionStatus, SnapshotDisposition,
};

const THREAD: &str = "thread_proj_thread";
const TURN_A: &str = "turn_a";
const TURN_B: &str = "turn_b";
const ITEM_USER: &str = "item_user";
const ITEM_ASSIST: &str = "item_assist";
const RUN: &str = "run_proj";

fn thread_id() -> ThreadId {
    ThreadId::parse(THREAD).expect("fixture thread id is valid")
}

fn stamp(millis: i64) -> UnixMillis {
    UnixMillis::from_millis(millis)
}

fn make_turn(
    id: &str,
    ordinal: u64,
    revision: u64,
    lifecycle: ConversationLifecycle,
) -> ConversationTurn {
    make_turn_at(id, ordinal, revision, lifecycle, -10, 20)
}

fn make_turn_at(
    id: &str,
    ordinal: u64,
    revision: u64,
    lifecycle: ConversationLifecycle,
    created_at: i64,
    updated_at: i64,
) -> ConversationTurn {
    ConversationTurn {
        turn_id: TurnId::parse(id).expect("fixture turn id is valid"),
        ordinal: TurnOrdinal::new(ordinal),
        revision: Revision::new(revision),
        lifecycle,
        created_at: stamp(created_at),
        updated_at: stamp(updated_at),
    }
}

/// Non-identity item fixture state, so builders keep few arguments and each
/// case states only the fields it changes.
#[derive(Clone, Copy)]
struct ItemState {
    revision: u64,
    lifecycle: ConversationLifecycle,
    created_at: i64,
    updated_at: i64,
}

impl ItemState {
    /// Revision zero, pending, created at -5, updated at 25.
    fn initial() -> Self {
        Self {
            revision: 0,
            lifecycle: ConversationLifecycle::Pending,
            created_at: -5,
            updated_at: 25,
        }
    }
}

fn make_user(id: &str, turn: &str, ordinal: u64, body: &str) -> ConversationItem {
    make_user_at(id, turn, ordinal, body, ItemState::initial())
}

fn make_user_at(
    id: &str,
    turn: &str,
    ordinal: u64,
    body: &str,
    state: ItemState,
) -> ConversationItem {
    ConversationItem::UserMessage(UserMessageItem {
        item_id: ItemId::parse(id).expect("fixture item id is valid"),
        turn_id: TurnId::parse(turn).expect("fixture turn id is valid"),
        ordinal: ItemOrdinal::new(ordinal),
        revision: Revision::new(state.revision),
        lifecycle: state.lifecycle,
        body: MessageBody::parse(body).expect("fixture body is valid"),
        created_at: stamp(state.created_at),
        updated_at: stamp(state.updated_at),
    })
}

fn make_assistant(
    id: &str,
    turn: &str,
    ordinal: u64,
    revision: u64,
    lifecycle: ConversationLifecycle,
    phase: AssistantMessagePhase,
    body: &str,
) -> ConversationItem {
    make_assistant_at(
        id,
        turn,
        ordinal,
        phase,
        body,
        ItemState {
            revision,
            lifecycle,
            ..ItemState::initial()
        },
    )
}

fn make_assistant_at(
    id: &str,
    turn: &str,
    ordinal: u64,
    phase: AssistantMessagePhase,
    body: &str,
    state: ItemState,
) -> ConversationItem {
    ConversationItem::AssistantMessage(AssistantMessageItem {
        item_id: ItemId::parse(id).expect("fixture item id is valid"),
        turn_id: TurnId::parse(turn).expect("fixture turn id is valid"),
        run_id: RunId::parse(RUN).expect("fixture run id is valid"),
        ordinal: ItemOrdinal::new(ordinal),
        revision: Revision::new(state.revision),
        lifecycle: state.lifecycle,
        body: AssistantBody::parse(body).expect("fixture body is valid"),
        phase,
        created_at: stamp(state.created_at),
        updated_at: stamp(state.updated_at),
    })
}

/// One maximum-size completed assistant body at `ordinal` under `TURN_A`.
fn full_body_assistant(id: &str, ordinal: u64) -> ConversationItem {
    make_assistant_at(
        id,
        TURN_A,
        ordinal,
        AssistantMessagePhase::Final,
        &"x".repeat(MESSAGE_BODY_MAX_BYTES),
        ItemState {
            lifecycle: ConversationLifecycle::Completed,
            ..ItemState::initial()
        },
    )
}

fn snapshot(
    cursor: u64,
    turns: Vec<ConversationTurn>,
    items: Vec<ConversationItem>,
    watermark: i64,
) -> ConversationSnapshot {
    ConversationSnapshot::new(
        thread_id(),
        ConversationCursor::new(cursor),
        turns,
        items,
        stamp(watermark),
    )
    .expect("fixture snapshot satisfies domain structure")
}

fn batch(from: u64, to: u64, patches: Vec<ConversationPatch>) -> PatchBatch {
    PatchBatch::new(
        thread_id(),
        ConversationCursor::new(from),
        ConversationCursor::new(to),
        patches,
    )
    .expect("fixture batch satisfies domain framing")
}

fn append_patch(
    sequence: u64,
    item: &str,
    revision: u64,
    text: &str,
    updated_at: i64,
) -> ConversationPatch {
    ConversationPatch::ItemAppend {
        patch_id: PatchId::parse(format!("p-append-{sequence}")).expect("fixture patch id"),
        sequence: PatchSequence::new(sequence).expect("positive"),
        item_id: ItemId::parse(item).expect("fixture item id is valid"),
        revision: Revision::new(revision),
        text: IncrementalText::parse(text).expect("fragment fits"),
        updated_at: stamp(updated_at),
    }
}

fn item_upsert_patch(sequence: u64, item: ConversationItem) -> ConversationPatch {
    ConversationPatch::ItemUpsert {
        patch_id: PatchId::parse(format!("p-upsert-{sequence}")).expect("fixture patch id"),
        sequence: PatchSequence::new(sequence).expect("positive"),
        item,
    }
}

fn turn_upsert_patch(sequence: u64, turn: ConversationTurn) -> ConversationPatch {
    ConversationPatch::TurnUpsert {
        patch_id: PatchId::parse(format!("p-turn-{sequence}")).expect("fixture patch id"),
        sequence: PatchSequence::new(sequence).expect("positive"),
        turn,
    }
}

fn item_lifecycle_patch(
    sequence: u64,
    item: &str,
    revision: u64,
    lifecycle: ConversationLifecycle,
    updated_at: i64,
) -> ConversationPatch {
    ConversationPatch::ItemLifecycle {
        patch_id: PatchId::parse(format!("p-ilc-{sequence}")).expect("fixture patch id"),
        sequence: PatchSequence::new(sequence).expect("positive"),
        item_id: ItemId::parse(item).expect("fixture item id is valid"),
        revision: Revision::new(revision),
        lifecycle,
        updated_at: stamp(updated_at),
    }
}

/// One-turn, one-user-item baseline installed at cursor four, watermark 30.
fn baseline_projection() -> ConversationProjection {
    let mut projection = ConversationProjection::new(thread_id());
    assert_eq!(
        projection.install_snapshot(&snapshot(
            4,
            vec![make_turn(TURN_A, 0, 0, ConversationLifecycle::Pending)],
            vec![make_user(ITEM_USER, TURN_A, 1, "Queued")],
            30,
        )),
        Ok(SnapshotDisposition::Applied)
    );
    assert_eq!(projection.status(), ProjectionStatus::Ready);
    projection
}

#[test]
fn exact_resumed_ack_clears_recovery_without_replacing_complete_snapshot() {
    let mut projection = baseline_projection();
    let before = projection.snapshot().cloned().expect("baseline is present");
    let before_ptr = projection
        .snapshot()
        .map(|snapshot| snapshot as *const ConversationSnapshot)
        .expect("baseline pointer is present");

    assert_eq!(
        projection.apply_batch(&batch(
            6,
            7,
            vec![append_patch(7, ITEM_USER, 1, "ignored", 31)],
        )),
        Err(ProjectionError::CursorMismatch)
    );
    assert_eq!(projection.status(), ProjectionStatus::ResnapshotRequired);
    assert_eq!(projection.snapshot(), Some(&before));

    assert_eq!(
        projection.acknowledge_resumed(&thread_id(), ConversationCursor::new(4)),
        Ok(())
    );
    assert_eq!(projection.status(), ProjectionStatus::Ready);
    assert_eq!(projection.snapshot(), Some(&before));
    assert_eq!(
        projection
            .snapshot()
            .expect("snapshot remains")
            .cursor()
            .get(),
        4
    );
    let after_ptr = projection
        .snapshot()
        .map(|snapshot| snapshot as *const ConversationSnapshot)
        .expect("snapshot pointer remains");
    assert_eq!(before_ptr, after_ptr, "resume only changes delivery health");
}

#[test]
fn exact_resumed_ack_is_idempotent_when_projection_is_ready() {
    let mut projection = baseline_projection();
    let before = projection.snapshot().cloned().expect("baseline is present");
    let before_ptr = projection
        .snapshot()
        .map(|snapshot| snapshot as *const ConversationSnapshot)
        .expect("baseline pointer is present");

    assert_eq!(
        projection.acknowledge_resumed(&thread_id(), ConversationCursor::new(4)),
        Ok(())
    );
    assert_eq!(projection.status(), ProjectionStatus::Ready);
    assert_eq!(projection.snapshot(), Some(&before));
    assert_eq!(
        projection.snapshot().map(ConversationSnapshot::cursor),
        Some(ConversationCursor::new(4))
    );

    assert_eq!(
        projection.acknowledge_resumed(&thread_id(), ConversationCursor::new(4)),
        Ok(())
    );
    assert_eq!(projection.status(), ProjectionStatus::Ready);
    assert_eq!(projection.snapshot(), Some(&before));
    let after_ptr = projection
        .snapshot()
        .map(|snapshot| snapshot as *const ConversationSnapshot)
        .expect("snapshot pointer remains");
    assert_eq!(before_ptr, after_ptr, "repeated resume remains status-only");
}

#[test]
fn resumed_ack_refusals_preserve_zero_visible_state_and_never_fake_readiness() {
    let mut empty = ConversationProjection::new(thread_id());
    assert_eq!(
        empty.acknowledge_resumed(&thread_id(), ConversationCursor::new(0)),
        Err(ProjectionError::BaselineRequired)
    );
    assert_eq!(empty.status(), ProjectionStatus::AwaitingSnapshot);
    assert!(empty.snapshot().is_none());

    let mut projection = baseline_projection();
    let before = projection.snapshot().cloned().expect("baseline is present");
    assert_eq!(
        projection.acknowledge_resumed(
            &ThreadId::parse("thread_foreign").expect("foreign thread is valid"),
            ConversationCursor::new(4),
        ),
        Err(ProjectionError::ThreadMismatch)
    );
    assert_eq!(projection.status(), ProjectionStatus::Ready);
    assert_eq!(projection.snapshot(), Some(&before));

    assert_eq!(
        projection.acknowledge_resumed(&thread_id(), ConversationCursor::new(3)),
        Err(ProjectionError::CursorMismatch)
    );
    assert_eq!(projection.status(), ProjectionStatus::Ready);
    assert_eq!(projection.snapshot(), Some(&before));

    projection
        .apply_batch(&batch(
            6,
            7,
            vec![append_patch(7, ITEM_USER, 1, "ignored", 31)],
        ))
        .expect_err("gap enters recovery");
    assert_eq!(projection.status(), ProjectionStatus::ResnapshotRequired);
    assert_eq!(
        projection.acknowledge_resumed(&thread_id(), ConversationCursor::new(3)),
        Err(ProjectionError::CursorMismatch)
    );
    assert_eq!(projection.status(), ProjectionStatus::ResnapshotRequired);
    assert_eq!(projection.snapshot(), Some(&before));
}

/// Cursor-20 window with two turns and both item kinds, used by the
/// common-entity immutability tests.
fn common_entity_base() -> ConversationSnapshot {
    snapshot(
        20,
        vec![
            make_turn(TURN_A, 0, 3, ConversationLifecycle::Completed),
            make_turn(TURN_B, 1, 1, ConversationLifecycle::Interrupted),
        ],
        vec![
            make_user(ITEM_USER, TURN_A, 2, "Queued"),
            make_assistant(
                ITEM_ASSIST,
                TURN_B,
                3,
                2,
                ConversationLifecycle::Streaming,
                AssistantMessagePhase::Final,
                "settled",
            ),
        ],
        100,
    )
}

/// Installs the common-entity base, then asserts `candidate` is refused
/// while the base window and ready status stay untouched.
fn expect_common_entity_conflict(candidate: &ConversationSnapshot) -> ProjectionError {
    let base = common_entity_base();
    let mut projection = ConversationProjection::new(thread_id());
    assert_eq!(
        projection.install_snapshot(&base),
        Ok(SnapshotDisposition::Applied)
    );
    let outcome = projection
        .install_snapshot(candidate)
        .expect_err("common-entity candidate must be refused");
    assert_eq!(
        projection.snapshot(),
        Some(&base),
        "failed installation preserves the prior window"
    );
    assert_eq!(projection.status(), ProjectionStatus::Ready);
    outcome
}

#[test]
fn uninitialized_owner_requires_baseline_and_filters_threads() {
    let mut projection = ConversationProjection::new(thread_id());
    assert_eq!(projection.status(), ProjectionStatus::AwaitingSnapshot);
    assert!(projection.snapshot().is_none());

    // Thread identity is checked before any state rule, so a batch naming a
    // different thread is a mismatch even on an uninitialized owner.
    let foreign_thread = ThreadId::parse("thread_foreign").expect("fixture is valid");
    let mut foreign_projection = ConversationProjection::new(foreign_thread);
    assert_eq!(
        foreign_projection.apply_batch(&batch(0, 1, vec![append_patch(1, ITEM_USER, 1, "x", 26)])),
        Err(ProjectionError::ThreadMismatch)
    );

    // A same-thread batch before any snapshot needs its baseline first and
    // stays awaiting; a wrong-thread batch never touches this owner at all.
    assert_eq!(
        projection.apply_batch(&batch(0, 1, vec![append_patch(1, ITEM_USER, 1, "x", 26)])),
        Err(ProjectionError::BaselineRequired)
    );
    let foreign_thread_snapshot = ConversationSnapshot::new(
        ThreadId::parse("thread_foreign").expect("fixture is valid"),
        ConversationCursor::default(),
        Vec::new(),
        Vec::new(),
        stamp(0),
    )
    .expect("foreign fixture is structurally valid");
    assert_eq!(
        projection.install_snapshot(&foreign_thread_snapshot),
        Err(ProjectionError::ThreadMismatch)
    );
    assert_eq!(
        foreign_projection.install_snapshot(&foreign_thread_snapshot),
        Ok(SnapshotDisposition::Applied)
    );
    assert_eq!(projection.status(), ProjectionStatus::AwaitingSnapshot);
    assert!(projection.snapshot().is_none());
}

#[test]
fn first_snapshot_accepts_arbitrary_revisions_and_canonical_order() {
    let mut projection = ConversationProjection::new(thread_id());
    let installed = projection
        .install_snapshot(&snapshot(
            9,
            vec![
                make_turn_at(TURN_B, 5, 7, ConversationLifecycle::Active, -60, -50),
                make_turn_at(TURN_A, 0, 2, ConversationLifecycle::Pending, -60, -45),
            ],
            vec![make_user_at(
                ITEM_USER,
                TURN_A,
                3,
                "Queued",
                ItemState {
                    revision: 4,
                    created_at: -55,
                    updated_at: -41,
                    ..ItemState::initial()
                },
            )],
            -40,
        ))
        .expect("first snapshot accepts arbitrary revisions/cursor/negatives");
    assert_eq!(installed, SnapshotDisposition::Applied);
    assert_eq!(projection.status(), ProjectionStatus::Ready);
    let materialized = projection.snapshot().expect("materialized");
    assert_eq!(materialized.cursor().get(), 9);
    let ordinals: Vec<u64> = materialized
        .turns()
        .iter()
        .map(|turn| turn.ordinal.get())
        .collect();
    assert_eq!(ordinals, vec![0, 5], "rows publish canonically sorted");
}

#[test]
fn snapshot_semantics_and_cross_kind_identity_are_enforced_atomically() {
    let mut projection = baseline_projection();
    let before = projection.snapshot().cloned().expect("baseline present");

    // Identical raw identifier text across a TurnId and an ItemId mirrors one
    // globally unique ledger entity_id column and must be rejected.
    let colliding = snapshot(
        5,
        vec![
            make_turn(TURN_A, 0, 0, ConversationLifecycle::Pending),
            make_turn(ITEM_USER, 2, 0, ConversationLifecycle::Pending),
        ],
        vec![make_user(ITEM_USER, TURN_A, 1, "Queued")],
        30,
    );
    assert_eq!(
        projection.install_snapshot(&colliding),
        Err(ProjectionError::IdentityConflict)
    );

    // Creation following update violates per-entity ordering.
    let backwards = snapshot(
        5,
        vec![make_turn_at(
            TURN_B,
            2,
            0,
            ConversationLifecycle::Pending,
            50,
            49,
        )],
        vec![],
        60,
    );
    assert_eq!(
        projection.install_snapshot(&backwards),
        Err(ProjectionError::TimeOrdering)
    );

    // An entity may never claim an update beyond its snapshot watermark.
    let beyond_watermark = snapshot(
        5,
        vec![make_turn(TURN_A, 0, 0, ConversationLifecycle::Pending)],
        vec![make_user_at(
            ITEM_ASSIST,
            TURN_A,
            2,
            "Queued",
            ItemState::initial(),
        )],
        24,
    );
    assert_eq!(
        projection.install_snapshot(&beyond_watermark),
        Err(ProjectionError::TimeOrdering)
    );

    assert_eq!(projection.snapshot(), Some(&before));
    assert_eq!(projection.status(), ProjectionStatus::Ready);
}

#[test]
fn retention_caps_preserve_prior_state_exactly() {
    // Exactly 128 maximum-size bodies land precisely on the 8 MiB budget.
    // Ordinal zero belongs to the turn, so item ordinals start at one.
    let exact_items: Vec<ConversationItem> = (0..128)
        .map(|index| full_body_assistant(&format!("assist_full_{index}"), index + 1))
        .collect();
    let exact = snapshot(
        4,
        vec![make_turn(TURN_A, 0, 0, ConversationLifecycle::Pending)],
        exact_items,
        30,
    );
    let mut exact_projection = ConversationProjection::new(thread_id());
    assert_eq!(
        exact_projection.install_snapshot(&exact),
        Ok(SnapshotDisposition::Applied)
    );

    // One more maximum-size body crosses the budget and preserves state.
    let mut over_items: Vec<ConversationItem> = (0..129)
        .map(|index| full_body_assistant(&format!("assist_over_{index}"), index + 1))
        .collect();
    over_items.push(make_user(ITEM_USER, TURN_A, 900, "tiny"));
    let over = snapshot(
        4,
        vec![make_turn(TURN_A, 0, 0, ConversationLifecycle::Pending)],
        over_items,
        30,
    );
    let mut over_projection = ConversationProjection::new(thread_id());
    assert_eq!(
        over_projection.install_snapshot(&over),
        Err(ProjectionError::RetentionExceeded)
    );
    assert!(over_projection.snapshot().is_none());
    assert_eq!(over_projection.status(), ProjectionStatus::AwaitingSnapshot);

    // The item-count ceiling behaves identically: one turn, 2049 items.
    let too_many_items: Vec<ConversationItem> = (0..2049)
        .map(|index| make_user(&format!("item_many_{index}"), TURN_A, index + 1, "tiny"))
        .collect();
    let too_many = snapshot(
        4,
        vec![make_turn(TURN_A, 0, 0, ConversationLifecycle::Pending)],
        too_many_items,
        30,
    );
    assert_eq!(
        over_projection.install_snapshot(&too_many),
        Err(ProjectionError::RetentionExceeded)
    );

    // A byte-budget-crossing batch insert enters recovery like every other
    // failure: the exact-fit window cannot absorb another maximum body.
    assert_eq!(
        exact_projection.apply_batch(&batch(
            4,
            5,
            vec![item_upsert_patch(
                5,
                full_body_assistant("assist_extra", 900)
            )],
        )),
        Err(ProjectionError::RetentionExceeded)
    );
    assert_eq!(
        exact_projection.status(),
        ProjectionStatus::ResnapshotRequired
    );
    assert_eq!(
        exact_projection
            .snapshot()
            .expect("preserved")
            .cursor()
            .get(),
        4
    );
}

#[test]
fn snapshot_cursor_ladder_rules_hold_exactly() {
    let mut projection = baseline_projection();

    let lower = snapshot(3, vec![], vec![], 30);
    assert_eq!(
        projection.install_snapshot(&lower),
        Err(ProjectionError::SnapshotConflict)
    );

    let equal_identical = snapshot(
        4,
        vec![make_turn(TURN_A, 0, 0, ConversationLifecycle::Pending)],
        vec![make_user(ITEM_USER, TURN_A, 1, "Queued")],
        30,
    );
    assert_eq!(
        projection.install_snapshot(&equal_identical),
        Ok(SnapshotDisposition::Unchanged)
    );
    let equal_conflicting = snapshot(
        4,
        vec![make_turn(TURN_A, 0, 0, ConversationLifecycle::Pending)],
        vec![make_user(ITEM_USER, TURN_A, 1, "Edited")],
        30,
    );
    assert_eq!(
        projection.install_snapshot(&equal_conflicting),
        Err(ProjectionError::SnapshotConflict)
    );

    let advanced = snapshot(
        12,
        vec![make_turn(TURN_B, 1, 4, ConversationLifecycle::Active)],
        vec![make_assistant(
            ITEM_ASSIST,
            TURN_B,
            2,
            1,
            ConversationLifecycle::Streaming,
            AssistantMessagePhase::Commentary,
            "partial",
        )],
        44,
    );
    assert_eq!(
        projection.install_snapshot(&advanced),
        Ok(SnapshotDisposition::Applied)
    );
    let materialized = projection.snapshot().expect("replaced");
    assert_eq!(materialized.cursor().get(), 12);
    assert_eq!(
        materialized.turns().len(),
        1,
        "omitted rows leave the window"
    );
    assert_eq!(materialized.items().len(), 1);
}

#[test]
fn snapshot_common_turns_keep_immutable_identity() {
    let with_turns = |turns: Vec<ConversationTurn>| snapshot(21, turns, vec![], 100);

    // Changed ordinal, changed creation, revision regression. A higher
    // window may omit unrelated rows entirely.
    assert_eq!(
        expect_common_entity_conflict(&with_turns(vec![make_turn(
            TURN_A,
            5,
            3,
            ConversationLifecycle::Completed
        )])),
        ProjectionError::IdentityConflict
    );
    assert_eq!(
        expect_common_entity_conflict(&with_turns(vec![make_turn_at(
            TURN_A,
            0,
            3,
            ConversationLifecycle::Completed,
            -9,
            20
        )])),
        ProjectionError::IdentityConflict
    );
    assert_eq!(
        expect_common_entity_conflict(&with_turns(vec![make_turn(
            TURN_A,
            0,
            2,
            ConversationLifecycle::Completed
        )])),
        ProjectionError::RevisionConflict
    );
}

#[test]
fn snapshot_common_items_keep_immutable_identity() {
    let with_item = |item: ConversationItem| {
        snapshot(
            21,
            vec![
                make_turn(TURN_A, 0, 3, ConversationLifecycle::Completed),
                make_turn(TURN_B, 1, 1, ConversationLifecycle::Interrupted),
            ],
            vec![item],
            100,
        )
    };

    // Kind swap under a known item identity.
    assert_eq!(
        expect_common_entity_conflict(&with_item(make_user(ITEM_ASSIST, TURN_B, 3, "settled"))),
        ProjectionError::IdentityConflict
    );
    // Parent turn swap.
    assert_eq!(
        expect_common_entity_conflict(&with_item(make_assistant(
            ITEM_ASSIST,
            TURN_A,
            3,
            3,
            ConversationLifecycle::Streaming,
            AssistantMessagePhase::Final,
            "settled",
        ))),
        ProjectionError::IdentityConflict
    );
    // Creation-instant change.
    assert_eq!(
        expect_common_entity_conflict(&with_item(make_user_at(
            ITEM_USER,
            TURN_A,
            2,
            "Queued",
            ItemState {
                created_at: -4,
                ..ItemState::initial()
            },
        ))),
        ProjectionError::IdentityConflict
    );
    // Equal-revision divergence is a conflict even when identities hold.
    assert_eq!(
        expect_common_entity_conflict(&with_item(make_user_at(
            ITEM_USER,
            TURN_A,
            2,
            "Queued",
            ItemState {
                lifecycle: ConversationLifecycle::Completed,
                updated_at: 99,
                ..ItemState::initial()
            },
        ))),
        ProjectionError::RevisionConflict
    );
}

#[test]
fn snapshot_ladders_watermark_and_legal_higher_windows_apply() {
    let base = snapshot(
        20,
        vec![
            make_turn(TURN_A, 0, 3, ConversationLifecycle::Failed),
            make_turn(TURN_B, 1, 1, ConversationLifecycle::Interrupted),
        ],
        vec![make_user(ITEM_USER, TURN_A, 2, "Queued")],
        100,
    );

    let mut projection = ConversationProjection::new(thread_id());
    assert_eq!(
        projection.install_snapshot(&base),
        Ok(SnapshotDisposition::Applied)
    );

    // Sealed terminal rejects a distinct lifecycle on a known turn.
    let sealed = snapshot(
        21,
        vec![
            make_turn(TURN_A, 0, 4, ConversationLifecycle::Streaming),
            make_turn(TURN_B, 1, 1, ConversationLifecycle::Interrupted),
        ],
        vec![make_user(ITEM_USER, TURN_A, 2, "Queued")],
        101,
    );
    assert_eq!(
        projection.install_snapshot(&sealed),
        Err(ProjectionError::Lifecycle(
            artisan_domain::LifecycleTransitionError::Sealed {
                from: ConversationLifecycle::Failed,
                to: ConversationLifecycle::Streaming,
            }
        ))
    );
    assert_eq!(projection.status(), ProjectionStatus::Ready);

    // Updated-time regression on a known entity is typed ordering failure.
    let regressed_time = snapshot(
        21,
        vec![
            make_turn_at(TURN_A, 0, 4, ConversationLifecycle::Failed, -10, 19),
            make_turn(TURN_B, 1, 1, ConversationLifecycle::Interrupted),
        ],
        vec![make_user(ITEM_USER, TURN_A, 2, "Queued")],
        101,
    );
    assert_eq!(
        projection.install_snapshot(&regressed_time),
        Err(ProjectionError::TimeOrdering)
    );

    // Watermark regression conflicts even with a higher cursor.
    let watermark_regression = snapshot(
        21,
        vec![
            make_turn(TURN_A, 0, 4, ConversationLifecycle::Failed),
            make_turn(TURN_B, 1, 1, ConversationLifecycle::Interrupted),
        ],
        vec![make_user(ITEM_USER, TURN_A, 2, "Queued")],
        99,
    );
    assert_eq!(
        projection.install_snapshot(&watermark_regression),
        Err(ProjectionError::SnapshotConflict)
    );

    // A fully legal higher window: revisions jump, Interrupted resumes,
    // updated times advance, and the watermark advances.
    let legal = snapshot(
        22,
        vec![
            make_turn_at(TURN_A, 0, 9, ConversationLifecycle::Failed, -10, 120),
            make_turn_at(TURN_B, 1, 2, ConversationLifecycle::Active, -10, 110),
        ],
        vec![make_user_at(
            ITEM_USER,
            TURN_A,
            2,
            "Queued",
            ItemState {
                revision: 5,
                lifecycle: ConversationLifecycle::Completed,
                updated_at: 119,
                ..ItemState::initial()
            },
        )],
        120,
    );
    assert_eq!(
        projection.install_snapshot(&legal),
        Ok(SnapshotDisposition::Applied)
    );
    assert_eq!(projection.snapshot().expect("advanced").cursor().get(), 22);
}

#[test]
fn batches_require_exact_continuation_and_stay_sticky_until_snapshot() {
    let mut projection = baseline_projection();

    // Correct continuation from 4 to 6 (first patch continues at 5).
    let applied = projection
        .apply_batch(&batch(
            4,
            6,
            vec![
                item_lifecycle_patch(5, ITEM_USER, 1, ConversationLifecycle::Completed, 28),
                append_patch(6, ITEM_USER, 2, " plus", 29),
            ],
        ))
        .expect("contiguous batch applies");
    assert_eq!(applied.to_cursor.get(), 6);

    // Exact replay of the already-applied frame requests resnapshot instead
    // of being silently ignored: subsumption is not payload equivalence.
    assert_eq!(
        projection.apply_batch(&batch(
            4,
            6,
            vec![
                item_lifecycle_patch(5, ITEM_USER, 1, ConversationLifecycle::Completed, 28),
                append_patch(6, ITEM_USER, 2, " plus", 29),
            ],
        )),
        Err(ProjectionError::CursorMismatch)
    );
    assert_eq!(projection.status(), ProjectionStatus::ResnapshotRequired);

    // While recovery is pending, no batch applies at all.
    assert_eq!(
        projection.apply_batch(&batch(6, 7, vec![append_patch(7, ITEM_USER, 3, "!", 31)])),
        Err(ProjectionError::RecoveryRequired)
    );

    // A valid equal-cursor identical snapshot clears recovery: rows and
    // watermark must match the published post-batch state exactly.
    let recovered = snapshot(
        6,
        vec![make_turn(TURN_A, 0, 0, ConversationLifecycle::Pending)],
        vec![make_user_at(
            ITEM_USER,
            TURN_A,
            1,
            "Queued plus",
            ItemState {
                revision: 2,
                lifecycle: ConversationLifecycle::Completed,
                updated_at: 29,
                ..ItemState::initial()
            },
        )],
        30,
    );
    assert_eq!(
        projection.install_snapshot(&recovered),
        Ok(SnapshotDisposition::Unchanged)
    );
    assert_eq!(projection.status(), ProjectionStatus::Ready);

    // Batches flow again from the restored cursor.
    let next = projection
        .apply_batch(&batch(6, 7, vec![append_patch(7, ITEM_USER, 3, "!", 31)]))
        .expect("post-recovery batch applies");
    assert_eq!(next.to_cursor.get(), 7);
}

#[test]
fn gaps_and_overlaps_request_resnapshot_without_hiding_conflicts() {
    let mut projection = baseline_projection();

    // Forward gap.
    assert_eq!(
        projection.apply_batch(&batch(
            6,
            8,
            vec![
                append_patch(7, ITEM_USER, 1, "x", 31),
                append_patch(8, ITEM_USER, 2, "y", 32)
            ],
        )),
        Err(ProjectionError::CursorMismatch)
    );
    assert_eq!(projection.status(), ProjectionStatus::ResnapshotRequired);

    let cleared = snapshot(
        4,
        vec![make_turn(TURN_A, 0, 0, ConversationLifecycle::Pending)],
        vec![make_user(ITEM_USER, TURN_A, 1, "Queued")],
        30,
    );
    assert_eq!(
        projection.install_snapshot(&cleared),
        Ok(SnapshotDisposition::Unchanged)
    );

    // A fully subsumed tail (entirely behind the current cursor) is also a
    // resnapshot request, never a silent ignore: subsumption proves nothing
    // about payload equivalence with zero retained history.
    assert_eq!(
        projection.apply_batch(&batch(3, 4, vec![append_patch(4, ITEM_USER, 1, "x", 31)],)),
        Err(ProjectionError::CursorMismatch)
    );
    assert_eq!(projection.status(), ProjectionStatus::ResnapshotRequired);
    assert_eq!(projection.snapshot().expect("preserved").cursor().get(), 4);
}

#[test]
fn invalid_suffix_rolls_back_every_staged_mutation() {
    let mut projection = baseline_projection();
    let before = projection.snapshot().cloned().expect("baseline present");

    let refused = projection.apply_batch(&batch(
        4,
        7,
        vec![
            item_lifecycle_patch(5, ITEM_USER, 1, ConversationLifecycle::Completed, 28),
            append_patch(6, ITEM_USER, 2, " kept?", 29),
            append_patch(7, "item_absent", 1, "?!", 30),
        ],
    ));
    assert_eq!(refused, Err(ProjectionError::UnknownTarget));
    assert_eq!(
        projection.snapshot(),
        Some(&before),
        "no staged prefix lands"
    );
    assert_eq!(projection.status(), ProjectionStatus::ResnapshotRequired);
}

#[test]
fn batch_upsert_ladders_reject_unknown_nonzero_and_jumps() {
    // A refused batch flips the owner into sticky recovery, so each refusal
    // case exercises its own fresh baseline.
    let expect_refusal = |patch: ConversationPatch, expected: ProjectionError| {
        let mut projection = baseline_projection();
        assert_eq!(
            projection.apply_batch(&batch(4, 5, vec![patch])),
            Err(expected)
        );
        assert_eq!(projection.status(), ProjectionStatus::ResnapshotRequired);
        assert_eq!(
            projection.snapshot().map(|window| window.cursor().get()),
            Some(4),
            "a refused batch preserves the prior window"
        );
    };

    // Unknown-entity upserts above revision zero need a fresh snapshot.
    expect_refusal(
        item_upsert_patch(
            5,
            make_user_at(
                ITEM_ASSIST,
                TURN_A,
                2,
                "fresh",
                ItemState {
                    revision: 3,
                    ..ItemState::initial()
                },
            ),
        ),
        ProjectionError::RevisionConflict,
    );
    // An inserted item needs its turn present at this patch position.
    expect_refusal(
        item_upsert_patch(5, make_user("item_orphan", TURN_B, 2, "orphan")),
        ProjectionError::UnknownTarget,
    );
    // Known entity, equal revision, changed value: conflict.
    expect_refusal(
        item_upsert_patch(5, make_user(ITEM_USER, TURN_A, 1, "Changed")),
        ProjectionError::RevisionConflict,
    );
    // Known entity jumping over unseen revisions requires a fresh snapshot.
    expect_refusal(
        item_upsert_patch(
            5,
            make_user_at(
                ITEM_USER,
                TURN_A,
                1,
                "Jumped",
                ItemState {
                    revision: 9,
                    lifecycle: ConversationLifecycle::Completed,
                    updated_at: 40,
                    ..ItemState::initial()
                },
            ),
        ),
        ProjectionError::RevisionConflict,
    );

    // Exact equal-revision replay applies harmlessly and advances the
    // cursor; checked_next continues; skipping past checked_next fails.
    let mut projection = baseline_projection();
    projection
        .apply_batch(&batch(
            4,
            5,
            vec![item_upsert_patch(
                5,
                make_user(ITEM_USER, TURN_A, 1, "Queued"),
            )],
        ))
        .expect("idempotent equal-revision upsert applies");
    assert_eq!(projection.snapshot().expect("current").cursor().get(), 5);
    let next_value = make_user_at(
        ITEM_USER,
        TURN_A,
        1,
        "Queued",
        ItemState {
            revision: 1,
            lifecycle: ConversationLifecycle::Completed,
            updated_at: 35,
            ..ItemState::initial()
        },
    );
    projection
        .apply_batch(&batch(5, 6, vec![item_upsert_patch(6, next_value)]))
        .expect("checked-next upsert applies");
    let skipping = make_user_at(
        ITEM_USER,
        TURN_A,
        1,
        "Queued",
        ItemState {
            revision: 3,
            lifecycle: ConversationLifecycle::Completed,
            updated_at: 36,
            ..ItemState::initial()
        },
    );
    assert_eq!(
        projection.apply_batch(&batch(6, 7, vec![item_upsert_patch(7, skipping)])),
        Err(ProjectionError::RevisionConflict)
    );
}

#[test]
fn patch_identity_immutability_and_time_rules_are_typed() {
    let seeded = snapshot(
        4,
        vec![make_turn(TURN_A, 0, 2, ConversationLifecycle::Streaming)],
        vec![make_assistant(
            ITEM_ASSIST,
            TURN_A,
            1,
            5,
            ConversationLifecycle::Streaming,
            AssistantMessagePhase::Commentary,
            "draft",
        )],
        40,
    );
    // A refused batch flips the owner into sticky recovery, so each case
    // reinstalls the seed into a fresh owner.
    let expect_refusal = |patch: ConversationPatch, expected: ProjectionError| {
        let mut projection = ConversationProjection::new(thread_id());
        projection.install_snapshot(&seeded).expect("seed installs");
        assert_eq!(
            projection.apply_batch(&batch(4, 5, vec![patch])),
            Err(expected)
        );
        assert_eq!(
            projection.snapshot(),
            Some(&seeded),
            "a refused batch preserves the prior window"
        );
        assert_eq!(projection.status(), ProjectionStatus::ResnapshotRequired);
    };
    let next_assistant = |turn: &str| {
        make_assistant_at(
            ITEM_ASSIST,
            turn,
            1,
            AssistantMessagePhase::Commentary,
            "draft2",
            ItemState {
                revision: 6,
                lifecycle: ConversationLifecycle::Streaming,
                updated_at: 41,
                ..ItemState::initial()
            },
        )
    };

    // Kind swap on a known item identity.
    expect_refusal(
        item_upsert_patch(5, make_user(ITEM_ASSIST, TURN_A, 1, "draft")),
        ProjectionError::IdentityConflict,
    );
    // Assistant run association is immutable.
    let mut run_swapped = next_assistant(TURN_A);
    if let ConversationItem::AssistantMessage(message) = &mut run_swapped {
        message.run_id = RunId::parse("run_swapped").expect("fixture run id is valid");
    }
    expect_refusal(
        item_upsert_patch(5, run_swapped),
        ProjectionError::IdentityConflict,
    );
    // Parent turn is immutable, even toward an absent turn.
    expect_refusal(
        item_upsert_patch(5, next_assistant("turn_absent_parent")),
        ProjectionError::IdentityConflict,
    );
    // Updated-time regression on an item lifecycle delta.
    expect_refusal(
        item_lifecycle_patch(5, ITEM_ASSIST, 6, ConversationLifecycle::Completed, 24),
        ProjectionError::TimeOrdering,
    );
    // Turn ordinal is immutable under a known-entity higher-revision upsert.
    expect_refusal(
        turn_upsert_patch(5, make_turn(TURN_A, 5, 3, ConversationLifecycle::Completed)),
        ProjectionError::IdentityConflict,
    );
    // Turn updated-time regression under a lifecycle delta is typed.
    expect_refusal(
        ConversationPatch::TurnLifecycle {
            patch_id: PatchId::parse("p-turn-lc").expect("fixture patch id"),
            sequence: PatchSequence::new(5).expect("positive"),
            turn_id: TurnId::parse(TURN_A).expect("fixture turn id is valid"),
            revision: Revision::new(3),
            lifecycle: ConversationLifecycle::Completed,
            updated_at: stamp(19),
        },
        ProjectionError::TimeOrdering,
    );
}

#[test]
fn append_semantics_are_generic_verbatim_and_bounded() {
    let mut projection = ConversationProjection::new(thread_id());
    projection
        .install_snapshot(&snapshot(
            4,
            vec![make_turn(TURN_A, 0, 0, ConversationLifecycle::Active)],
            vec![
                make_user(ITEM_USER, TURN_A, 1, "héllo "),
                make_assistant_at(
                    ITEM_ASSIST,
                    TURN_A,
                    2,
                    AssistantMessagePhase::Final,
                    "",
                    ItemState {
                        lifecycle: ConversationLifecycle::Streaming,
                        ..ItemState::initial()
                    },
                ),
            ],
            30,
        ))
        .expect("seed installs");

    // Multibyte user-body append stays byte-exact.
    projection
        .apply_batch(&batch(
            4,
            5,
            vec![append_patch(5, ITEM_USER, 1, "🦀 world", 31)],
        ))
        .expect("multibyte append applies");
    let items = projection.snapshot().expect("current").items();
    let ConversationItem::UserMessage(user) = &items[0] else {
        panic!("user row stays first");
    };
    assert_eq!(user.body.as_str(), "héllo 🦀 world");
    assert_eq!(user.revision.get(), 1);

    // An empty opening append onto an empty assistant body is legal and
    // still consumes exactly one revision.
    projection
        .apply_batch(&batch(5, 6, vec![append_patch(6, ITEM_ASSIST, 1, "", 32)]))
        .expect("empty append applies");
    let items = projection.snapshot().expect("current").items();
    let ConversationItem::AssistantMessage(assistant) = &items[1] else {
        panic!("assistant row stays second");
    };
    assert_eq!(assistant.body.as_str(), "");
    assert_eq!(assistant.revision.get(), 1);

    // Grow the assistant body to exactly the ceiling: sixteen appends walk
    // revisions 2..=17 and cursors 7..=22 after the empty append used one.
    for index in 0..16u64 {
        projection
            .apply_batch(&batch(
                6 + index,
                7 + index,
                vec![append_patch(
                    7 + index,
                    ITEM_ASSIST,
                    2 + index,
                    &"a".repeat(4096),
                    33,
                )],
            ))
            .expect("boundary growth applies");
    }
    let full = projection.snapshot().expect("current");
    assert_eq!(full.cursor().get(), 22);
    let ConversationItem::AssistantMessage(grown) = &full.items()[1] else {
        panic!("assistant row stays second");
    };
    assert_eq!(grown.body.as_str().len(), MESSAGE_BODY_MAX_BYTES);
    assert_eq!(grown.revision.get(), 17);

    // One more byte continues the exact ladder (22 -> 23, revision 18), so
    // the refusal is the body bound itself, never a cursor or revision
    // mismatch; the caller's batch and the pre-failure window survive
    // verbatim, and a valid identical snapshot then recovers delivery.
    let before = projection.snapshot().cloned().expect("current");
    let over_limit = batch(22, 23, vec![append_patch(23, ITEM_ASSIST, 18, "!", 34)]);
    assert_eq!(
        projection.apply_batch(&over_limit),
        Err(ProjectionError::BodyBoundExceeded)
    );
    assert_eq!(
        over_limit,
        batch(22, 23, vec![append_patch(23, ITEM_ASSIST, 18, "!", 34)]),
        "caller input is never mutated"
    );
    assert_eq!(
        projection.snapshot(),
        Some(&before),
        "every staged mutation rolls back"
    );
    assert_eq!(projection.status(), ProjectionStatus::ResnapshotRequired);
    assert_eq!(
        projection.install_snapshot(&before),
        Ok(SnapshotDisposition::Unchanged)
    );
    assert_eq!(projection.status(), ProjectionStatus::Ready);
}

#[test]
fn watermark_tracks_delta_maximum_without_restamping_full_values() {
    let mut projection = baseline_projection();

    // Delta times below the current watermark leave it unchanged.
    projection
        .apply_batch(&batch(
            4,
            5,
            vec![item_lifecycle_patch(
                5,
                ITEM_USER,
                1,
                ConversationLifecycle::Completed,
                29,
            )],
        ))
        .expect("older-stamped delta applies");
    assert_eq!(
        projection.snapshot().expect("current").updated_at(),
        stamp(30)
    );

    // A newer delta raises the watermark; the full value keeps its own time.
    // The sealed terminal lifecycle replays identically, isolating the time
    // rule from the transition rule.
    projection
        .apply_batch(&batch(
            5,
            6,
            vec![item_lifecycle_patch(
                6,
                ITEM_USER,
                2,
                ConversationLifecycle::Completed,
                77,
            )],
        ))
        .expect("same-terminal delta applies");
    assert_eq!(
        projection.snapshot().expect("current").updated_at(),
        stamp(77)
    );

    // Full upserts contribute their own supplied instants to the watermark
    // maximum, and rows keep independent metadata: the turn's newer time
    // raises the watermark, the fresh assistant row keeps its older time,
    // and neither existing row is restamped with the maximum.
    projection
        .apply_batch(&batch(
            6,
            8,
            vec![
                turn_upsert_patch(
                    7,
                    make_turn_at(TURN_A, 0, 1, ConversationLifecycle::Active, -10, 90),
                ),
                item_upsert_patch(
                    8,
                    make_assistant_at(
                        ITEM_ASSIST,
                        TURN_A,
                        2,
                        AssistantMessagePhase::Commentary,
                        "spark",
                        ItemState {
                            lifecycle: ConversationLifecycle::Streaming,
                            updated_at: 50,
                            ..ItemState::initial()
                        },
                    ),
                ),
            ],
        ))
        .expect("full upserts apply");
    let materialized = projection.snapshot().expect("current");
    assert_eq!(materialized.updated_at(), stamp(90));
    assert_eq!(materialized.turns()[0].updated_at, stamp(90));
    let ConversationItem::UserMessage(user) = &materialized.items()[0] else {
        panic!("user row stays first");
    };
    assert_eq!(user.updated_at, stamp(77), "delta rows keep their own time");
    let ConversationItem::AssistantMessage(assistant) = &materialized.items()[1] else {
        panic!("assistant row is second");
    };
    assert_eq!(
        assistant.updated_at,
        stamp(50),
        "a full value below the watermark is never restamped upward"
    );
}

#[test]
fn phase_final_and_turn_completion_prove_nothing_about_completion() {
    let mut projection = ConversationProjection::new(thread_id());
    projection
        .install_snapshot(&snapshot(
            4,
            vec![make_turn(TURN_A, 0, 0, ConversationLifecycle::Streaming)],
            vec![make_assistant(
                ITEM_ASSIST,
                TURN_A,
                1,
                0,
                ConversationLifecycle::Streaming,
                AssistantMessagePhase::Final,
                "",
            )],
            30,
        ))
        .expect("seed installs");

    // Final-phase text streams while the item is still merely Streaming.
    projection
        .apply_batch(&batch(
            4,
            5,
            vec![append_patch(5, ITEM_ASSIST, 1, "final words", 31)],
        ))
        .expect("final-phase append applies");
    let items = projection.snapshot().expect("current").items();
    let ConversationItem::AssistantMessage(message) = &items[0] else {
        panic!("assistant row expected");
    };
    assert_eq!(message.phase, AssistantMessagePhase::Final);
    assert_eq!(message.lifecycle, ConversationLifecycle::Streaming);

    // Sealing the item lifecycle does not touch the disclosed phase, and a
    // same-terminal higher-revision correction afterwards stays legal.
    projection
        .apply_batch(&batch(
            5,
            6,
            vec![item_lifecycle_patch(
                6,
                ITEM_ASSIST,
                2,
                ConversationLifecycle::Completed,
                32,
            )],
        ))
        .expect("completion transition applies");
    projection
        .apply_batch(&batch(
            6,
            7,
            vec![item_upsert_patch(
                7,
                make_assistant_at(
                    ITEM_ASSIST,
                    TURN_A,
                    1,
                    AssistantMessagePhase::Final,
                    "final words (corrected)",
                    ItemState {
                        revision: 3,
                        lifecycle: ConversationLifecycle::Completed,
                        updated_at: 40,
                        ..ItemState::initial()
                    },
                ),
            )],
        ))
        .expect("same-terminal correction applies");
    let items = projection.snapshot().expect("current").items();
    let ConversationItem::AssistantMessage(message) = &items[0] else {
        panic!("assistant row expected");
    };
    assert_eq!(message.phase, AssistantMessagePhase::Final);
    assert_eq!(message.lifecycle, ConversationLifecycle::Completed);
    assert_eq!(message.body.as_str(), "final words (corrected)");
}

// ---- Cross-window kind reincarnation (both directions) ----

#[test]
fn cross_window_turn_reincarnated_as_item_is_rejected() {
    let previous = snapshot(
        20,
        vec![make_turn(TURN_A, 0, 0, ConversationLifecycle::Pending)],
        vec![make_user(ITEM_USER, TURN_A, 1, "Queued")],
        100,
    );
    let mut projection = ConversationProjection::new(thread_id());
    assert_eq!(
        projection.install_snapshot(&previous),
        Ok(SnapshotDisposition::Applied)
    );
    // Higher window omits turn_a and reintroduces its raw id as an item.
    let incoming = snapshot(
        21,
        vec![make_turn(TURN_B, 10, 0, ConversationLifecycle::Pending)],
        vec![make_user(TURN_A, TURN_B, 11, "hijack")],
        101,
    );
    assert_eq!(
        projection.install_snapshot(&incoming),
        Err(ProjectionError::IdentityConflict)
    );
    assert_eq!(projection.snapshot(), Some(&previous));
    assert_eq!(projection.status(), ProjectionStatus::Ready);
    assert_eq!(projection.snapshot().expect("preserved").cursor().get(), 20);
    assert_eq!(
        projection.snapshot().expect("preserved").updated_at(),
        stamp(100)
    );
}

#[test]
fn cross_window_item_reincarnated_as_turn_is_rejected() {
    let previous = snapshot(
        20,
        vec![make_turn(TURN_A, 0, 0, ConversationLifecycle::Pending)],
        vec![make_user(ITEM_USER, TURN_A, 1, "Queued")],
        100,
    );
    let mut projection = ConversationProjection::new(thread_id());
    assert_eq!(
        projection.install_snapshot(&previous),
        Ok(SnapshotDisposition::Applied)
    );
    let incoming = snapshot(
        21,
        vec![
            make_turn(TURN_A, 0, 0, ConversationLifecycle::Pending),
            make_turn(ITEM_USER, 12, 0, ConversationLifecycle::Pending),
        ],
        vec![],
        101,
    );
    assert_eq!(
        projection.install_snapshot(&incoming),
        Err(ProjectionError::IdentityConflict)
    );
    assert_eq!(projection.snapshot(), Some(&previous));
    assert_eq!(projection.status(), ProjectionStatus::Ready);
}

#[test]
fn independent_item_upsert_raises_watermark_without_restamping() {
    let mut projection = baseline_projection();
    let before_turn_updated = projection.snapshot().expect("baseline").turns()[0].updated_at;
    // ItemUpsert alone raises watermark; turn row keeps its own timestamp.
    projection
        .apply_batch(&batch(
            4,
            5,
            vec![item_upsert_patch(
                5,
                make_assistant_at(
                    ITEM_ASSIST,
                    TURN_A,
                    2,
                    AssistantMessagePhase::Commentary,
                    "spark",
                    ItemState {
                        lifecycle: ConversationLifecycle::Streaming,
                        updated_at: 90,
                        ..ItemState::initial()
                    },
                ),
            )],
        ))
        .expect("item upsert applies");
    let materialized = projection.snapshot().expect("current");
    assert_eq!(materialized.updated_at(), stamp(90));
    assert_eq!(
        materialized.turns()[0].updated_at,
        before_turn_updated,
        "other rows keep their own timestamps"
    );
    let ConversationItem::AssistantMessage(assistant) = &materialized.items()[1] else {
        panic!("assistant row is second");
    };
    assert_eq!(assistant.updated_at, stamp(90));
    let ConversationItem::UserMessage(user) = &materialized.items()[0] else {
        panic!("user row stays first");
    };
    assert_eq!(user.updated_at, stamp(25));
}

#[test]
fn revision_overflow_checked_next_is_rejected_with_rollback() {
    let max = u64::MAX;
    let seed = snapshot(
        4,
        vec![make_turn(TURN_A, 0, max, ConversationLifecycle::Streaming)],
        vec![make_assistant(
            ITEM_ASSIST,
            TURN_A,
            1,
            max,
            ConversationLifecycle::Streaming,
            AssistantMessagePhase::Commentary,
            "draft",
        )],
        40,
    );
    // Identical equal-revision upsert at MAX is legal and advances cursor.
    let mut projection = ConversationProjection::new(thread_id());
    assert_eq!(
        projection.install_snapshot(&seed),
        Ok(SnapshotDisposition::Applied)
    );
    assert_eq!(
        projection.apply_batch(&batch(
            4,
            5,
            vec![item_upsert_patch(
                5,
                make_assistant(
                    ITEM_ASSIST,
                    TURN_A,
                    1,
                    max,
                    ConversationLifecycle::Streaming,
                    AssistantMessagePhase::Commentary,
                    "draft",
                ),
            )],
        )),
        Ok(
            artisan_frontend::conversation_projection::BatchDisposition {
                to_cursor: ConversationCursor::new(5)
            }
        )
    );
    assert_eq!(projection.status(), ProjectionStatus::Ready);

    // Overflow: next revision would be MAX+1 via checked_next.
    let mut overflow = ConversationProjection::new(thread_id());
    assert_eq!(
        overflow.install_snapshot(&seed),
        Ok(SnapshotDisposition::Applied)
    );
    let before = overflow.snapshot().cloned().expect("seed present");
    assert_eq!(
        overflow.apply_batch(&batch(4, 5, vec![append_patch(5, ITEM_ASSIST, 0, "x", 41)])),
        Err(ProjectionError::RevisionConflict)
    );
    assert_eq!(overflow.snapshot(), Some(&before));
    assert_eq!(overflow.status(), ProjectionStatus::ResnapshotRequired);
    assert_eq!(overflow.snapshot().expect("preserved").cursor().get(), 4);
}

fn build_turns(count: usize, prefix: &str) -> Vec<ConversationTurn> {
    (0..count)
        .map(|i| {
            make_turn(
                &format!("{prefix}_{i}"),
                i as u64,
                0,
                ConversationLifecycle::Pending,
            )
        })
        .collect()
}

fn build_items(
    count: usize,
    prefix: &str,
    turn: &str,
    start_ordinal: u64,
) -> Vec<ConversationItem> {
    (0..count)
        .map(|i| {
            make_user(
                &format!("{prefix}_{i}"),
                turn,
                start_ordinal + i as u64,
                "x",
            )
        })
        .collect()
}

#[test]
fn exact_512_turn_boundary_and_next_row_refusal() {
    let turns_512 = build_turns(512, "turn_exact");
    let snap_512 = ConversationSnapshot::new(
        thread_id(),
        ConversationCursor::new(10),
        turns_512,
        Vec::new(),
        stamp(50),
    )
    .expect("512 turns is exactly the ceiling");
    let mut projection = ConversationProjection::new(thread_id());
    assert_eq!(
        projection.install_snapshot(&snap_512),
        Ok(SnapshotDisposition::Applied)
    );
    assert_eq!(projection.status(), ProjectionStatus::Ready);
    let before = projection.snapshot().cloned().expect("512 present");
    assert_eq!(
        projection.apply_batch(&batch(
            10,
            11,
            vec![turn_upsert_patch(
                11,
                make_turn("turn_extra", 512, 0, ConversationLifecycle::Pending)
            )]
        )),
        Err(ProjectionError::RetentionExceeded)
    );
    assert_eq!(projection.snapshot(), Some(&before));
    assert_eq!(projection.status(), ProjectionStatus::ResnapshotRequired);
    assert_eq!(projection.snapshot().expect("preserved").cursor().get(), 10);
    assert_eq!(
        projection.snapshot().expect("preserved").updated_at(),
        stamp(50)
    );
}

#[test]
fn exact_2048_item_boundary_and_next_row_refusal() {
    let one_turn = vec![make_turn(TURN_A, 0, 0, ConversationLifecycle::Pending)];
    let items_2048 = build_items(2048, "item_exact", TURN_A, 1);
    let snap_2048 = snapshot(20, one_turn.clone(), items_2048, 60);
    let mut projection = ConversationProjection::new(thread_id());
    assert_eq!(
        projection.install_snapshot(&snap_2048),
        Ok(SnapshotDisposition::Applied)
    );
    let before = projection.snapshot().cloned().expect("2048 present");
    assert_eq!(
        projection.apply_batch(&batch(
            20,
            21,
            vec![item_upsert_patch(
                21,
                make_user("item_extra", TURN_A, 2049, "x")
            )]
        )),
        Err(ProjectionError::RetentionExceeded)
    );
    assert_eq!(projection.snapshot(), Some(&before));
    assert_eq!(projection.status(), ProjectionStatus::ResnapshotRequired);
    assert_eq!(projection.snapshot().expect("preserved").cursor().get(), 20);
    assert_eq!(
        projection.snapshot().expect("preserved").updated_at(),
        stamp(60)
    );
    // Snapshot-level 2049 also exceeds retention.
    let over_items = build_items(2049, "item_over", TURN_A, 1);
    let snap_over = ConversationSnapshot::new(
        thread_id(),
        ConversationCursor::new(21),
        one_turn,
        over_items,
        stamp(61),
    )
    .expect("domain permits 2049 structurally");
    let mut fresh = ConversationProjection::new(thread_id());
    assert_eq!(
        fresh.install_snapshot(&snap_2048),
        Ok(SnapshotDisposition::Applied)
    );
    assert_eq!(
        fresh.install_snapshot(&snap_over),
        Err(ProjectionError::RetentionExceeded)
    );
    assert_eq!(fresh.snapshot().expect("preserved").cursor().get(), 20);
}

#[test]
fn body_replacement_shorter_then_longer_accounts_exactly() {
    // Seed exactly at 8 MiB with 128 max assistant bodies.
    let exact_items: Vec<ConversationItem> = (0..128)
        .map(|i| full_body_assistant(&format!("rep_full_{i}"), i + 1))
        .collect();
    let exact = snapshot(
        30,
        vec![make_turn(TURN_A, 0, 0, ConversationLifecycle::Pending)],
        exact_items,
        100,
    );
    let mut projection = ConversationProjection::new(thread_id());
    assert_eq!(
        projection.install_snapshot(&exact),
        Ok(SnapshotDisposition::Applied)
    );
    // Shorter replacement: same assistant identity, smaller body.
    let shorter = make_assistant_at(
        "rep_full_0",
        TURN_A,
        1,
        AssistantMessagePhase::Final,
        "tiny",
        ItemState {
            revision: 1,
            lifecycle: ConversationLifecycle::Completed,
            updated_at: 101,
            ..ItemState::initial()
        },
    );
    assert_eq!(
        projection.apply_batch(&batch(30, 31, vec![item_upsert_patch(31, shorter)])),
        Ok(
            artisan_frontend::conversation_projection::BatchDisposition {
                to_cursor: ConversationCursor::new(31)
            }
        )
    );
    assert_eq!(projection.snapshot().expect("current").cursor().get(), 31);
    assert_eq!(
        projection.snapshot().expect("current").updated_at(),
        stamp(101)
    );
    let ConversationItem::AssistantMessage(after_shorter) = projection
        .snapshot()
        .expect("current")
        .items()
        .iter()
        .find(|item| item.item_id().as_str() == "rep_full_0")
        .expect("replaced row present")
    else {
        panic!("expected assistant row");
    };
    assert_eq!(after_shorter.body.as_str(), "tiny");
    assert_eq!(after_shorter.revision.get(), 1);

    // Longer replacement back to exact budget is accepted.
    let longer = make_assistant_at(
        "rep_full_0",
        TURN_A,
        1,
        AssistantMessagePhase::Final,
        &"x".repeat(MESSAGE_BODY_MAX_BYTES),
        ItemState {
            revision: 2,
            lifecycle: ConversationLifecycle::Completed,
            updated_at: 102,
            ..ItemState::initial()
        },
    );
    assert_eq!(
        projection.apply_batch(&batch(31, 32, vec![item_upsert_patch(32, longer)])),
        Ok(
            artisan_frontend::conversation_projection::BatchDisposition {
                to_cursor: ConversationCursor::new(32)
            }
        )
    );
    assert_eq!(projection.snapshot().expect("current").cursor().get(), 32);
    assert_eq!(
        projection.snapshot().expect("current").updated_at(),
        stamp(102)
    );
}

#[test]
fn body_replacement_over_budget_is_rejected_with_full_rollback() {
    // 127 max + 2 tiny items is under budget. Replacing one tiny with max would be 8 MiB + 4.
    let mut seed_items: Vec<ConversationItem> = (0..127)
        .map(|i| full_body_assistant(&format!("over_full_{i}"), i + 1))
        .collect();
    seed_items.push(make_user("tiny_a", TURN_A, 128, "tiny"));
    seed_items.push(make_user("tiny_b", TURN_A, 129, "tiny"));
    let seed = snapshot(
        40,
        vec![make_turn(TURN_A, 0, 0, ConversationLifecycle::Pending)],
        seed_items,
        200,
    );
    let mut projection = ConversationProjection::new(thread_id());
    assert_eq!(
        projection.install_snapshot(&seed),
        Ok(SnapshotDisposition::Applied)
    );
    let before = projection.snapshot().cloned().expect("seed present");
    // Build growing replacement preserving the tiny row's turn/ordinal/created kind,
    // but as a user message (same kind) with larger body. Use make_user_at to keep kind.
    let growing = make_user_at(
        "tiny_a",
        TURN_A,
        128,
        &"x".repeat(MESSAGE_BODY_MAX_BYTES),
        ItemState {
            revision: 1,
            updated_at: 201,
            ..ItemState::initial()
        },
    );
    assert_eq!(
        projection.apply_batch(&batch(40, 41, vec![item_upsert_patch(41, growing)])),
        Err(ProjectionError::RetentionExceeded)
    );
    assert_eq!(projection.snapshot(), Some(&before));
    assert_eq!(projection.status(), ProjectionStatus::ResnapshotRequired);
    assert_eq!(projection.snapshot().expect("preserved").cursor().get(), 40);
    assert_eq!(
        projection.snapshot().expect("preserved").updated_at(),
        stamp(200)
    );
}
