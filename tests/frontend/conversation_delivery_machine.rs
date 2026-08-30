//! Black-box coverage for the Statig conversation delivery machine.

use artisan_domain::{
    AssistantBody, AssistantMessageItem, AssistantMessagePhase, ConversationCursor,
    ConversationItem, ConversationLifecycle, ConversationPatch, ConversationSnapshot,
    ConversationTurn, IncrementalText, ItemId, ItemOrdinal, MessageBody, PatchBatch, PatchId,
    PatchSequence, Revision, ThreadId, TurnId, TurnOrdinal, UnixMillis, UserMessageItem,
};
use artisan_frontend::conversation_delivery_machine::{
    ConversationDeliveryController, ConversationDeliveryEffect, ConversationDeliveryError,
    DeliveryPhase,
};
use artisan_frontend::conversation_projection::ProjectionStatus;

const THREAD: &str = "thread_delivery_machine";
const TURN_A: &str = "turn_delivery_a";
const ITEM_USER: &str = "item_delivery_user";
const ITEM_ASSIST: &str = "item_delivery_assist";

fn thread_id() -> ThreadId {
    ThreadId::parse(THREAD).expect("fixture thread id is valid")
}

fn foreign_thread_id() -> ThreadId {
    ThreadId::parse("thread_delivery_foreign").expect("foreign thread id is valid")
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
    ConversationTurn {
        turn_id: TurnId::parse(id).expect("fixture turn id is valid"),
        ordinal: TurnOrdinal::new(ordinal),
        revision: Revision::new(revision),
        lifecycle,
        created_at: stamp(-10),
        updated_at: stamp(20),
    }
}

fn make_user(id: &str, turn: &str, ordinal: u64, body: &str) -> ConversationItem {
    ConversationItem::UserMessage(UserMessageItem {
        item_id: ItemId::parse(id).expect("fixture item id"),
        turn_id: TurnId::parse(turn).expect("fixture turn id"),
        ordinal: ItemOrdinal::new(ordinal),
        revision: Revision::new(0),
        lifecycle: ConversationLifecycle::Pending,
        body: MessageBody::parse(body).expect("fixture body"),
        created_at: stamp(-5),
        updated_at: stamp(25),
    })
}

fn make_assistant(id: &str, turn: &str, ordinal: u64, body: &str) -> ConversationItem {
    ConversationItem::AssistantMessage(AssistantMessageItem {
        item_id: ItemId::parse(id).expect("fixture item id"),
        turn_id: TurnId::parse(turn).expect("fixture turn id"),
        run_id: artisan_domain::RunId::parse("run_delivery").expect("run id"),
        ordinal: ItemOrdinal::new(ordinal),
        revision: Revision::new(0),
        lifecycle: ConversationLifecycle::Pending,
        body: AssistantBody::parse(body).expect("fixture body"),
        phase: AssistantMessagePhase::Final,
        created_at: stamp(-5),
        updated_at: stamp(25),
    })
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
    .expect("fixture snapshot valid")
}

fn foreign_snapshot() -> ConversationSnapshot {
    ConversationSnapshot::new(
        foreign_thread_id(),
        ConversationCursor::new(0),
        Vec::new(),
        Vec::new(),
        stamp(0),
    )
    .expect("foreign snapshot valid")
}

fn batch(from: u64, to: u64, patches: Vec<ConversationPatch>) -> PatchBatch {
    PatchBatch::new(
        thread_id(),
        ConversationCursor::new(from),
        ConversationCursor::new(to),
        patches,
    )
    .expect("fixture batch valid")
}

fn foreign_batch(from: u64, to: u64) -> PatchBatch {
    PatchBatch::new(
        foreign_thread_id(),
        ConversationCursor::new(from),
        ConversationCursor::new(to),
        vec![ConversationPatch::ItemAppend {
            patch_id: PatchId::parse("p-foreign").expect("patch id"),
            sequence: PatchSequence::new(to).expect("positive"),
            item_id: ItemId::parse(ITEM_USER).expect("item id"),
            revision: Revision::new(1),
            text: IncrementalText::parse("x").expect("fragment"),
            updated_at: stamp(30),
        }],
    )
    .expect("foreign batch valid")
}

fn baseline_snapshot() -> ConversationSnapshot {
    snapshot(
        4,
        vec![make_turn(TURN_A, 0, 0, ConversationLifecycle::Pending)],
        vec![make_user(ITEM_USER, TURN_A, 1, "Queued")],
        30,
    )
}

fn append_patch(sequence: u64, item: &str, revision: u64, text: &str) -> ConversationPatch {
    ConversationPatch::ItemAppend {
        patch_id: PatchId::parse(format!("p-append-{sequence}")).expect("patch id"),
        sequence: PatchSequence::new(sequence).expect("positive"),
        item_id: ItemId::parse(item).expect("item id"),
        revision: Revision::new(revision),
        text: IncrementalText::parse(text).expect("fragment"),
        updated_at: stamp(31),
    }
}

fn lifecycle_patch(
    sequence: u64,
    item: &str,
    revision: u64,
    lifecycle: ConversationLifecycle,
) -> ConversationPatch {
    ConversationPatch::ItemLifecycle {
        patch_id: PatchId::parse(format!("p-lc-{sequence}")).expect("patch id"),
        sequence: PatchSequence::new(sequence).expect("positive"),
        item_id: ItemId::parse(item).expect("item id"),
        revision: Revision::new(revision),
        lifecycle,
        updated_at: stamp(32),
    }
}

fn is_request(effect: &ConversationDeliveryEffect) -> bool {
    matches!(effect, ConversationDeliveryEffect::RequestSnapshot { .. })
}

fn is_invalidate(effect: &ConversationDeliveryEffect) -> bool {
    matches!(effect, ConversationDeliveryEffect::Invalidate)
}

fn is_report(effect: &ConversationDeliveryEffect) -> bool {
    matches!(effect, ConversationDeliveryEffect::ReportRefusal { .. })
}

fn request_generation(effect: &ConversationDeliveryEffect) -> Option<u64> {
    match effect {
        ConversationDeliveryEffect::RequestSnapshot { generation, .. } => Some(*generation),
        _ => None,
    }
}

fn request_after(effect: &ConversationDeliveryEffect) -> Option<Option<ConversationCursor>> {
    match effect {
        ConversationDeliveryEffect::RequestSnapshot { after, .. } => Some(*after),
        _ => None,
    }
}

// 1. construction enters awaiting and emits exactly one baseline request with generation one
#[test]
fn construction_enters_awaiting_and_emits_baseline_request() {
    let mut controller = ConversationDeliveryController::new(thread_id());
    assert_eq!(controller.phase(), DeliveryPhase::AwaitingSnapshot);
    assert_eq!(
        controller.projection_status(),
        ProjectionStatus::AwaitingSnapshot
    );
    assert!(controller.snapshot().is_none());
    let effects = controller.drain_effects();
    assert_eq!(effects.len(), 1, "exactly one baseline request");
    match &effects[0] {
        ConversationDeliveryEffect::RequestSnapshot {
            thread_id: tid,
            generation,
            after,
        } => {
            assert_eq!(tid, &thread_id());
            assert_eq!(*generation, 1);
            assert_eq!(*after, None);
        }
        other => panic!("expected RequestSnapshot, got {other:?}"),
    }
    assert_eq!(controller.view().pending_effects, 0);
    assert_eq!(controller.view().phase, DeliveryPhase::AwaitingSnapshot);
}

// 2. baseline snapshot enters ready and exposes canonical snapshot
#[test]
fn baseline_snapshot_enters_ready_and_exposes_canonical_snapshot() {
    let mut controller = ConversationDeliveryController::new(thread_id());
    let _ = controller.drain_effects();
    let baseline = baseline_snapshot();
    controller
        .on_snapshot(baseline.clone())
        .expect("baseline snapshot should apply");
    assert_eq!(controller.phase(), DeliveryPhase::Ready);
    assert_eq!(controller.projection_status(), ProjectionStatus::Ready);
    let materialized = controller.snapshot().expect("snapshot present");
    assert_eq!(materialized.cursor().get(), 4);
    let effects = controller.drain_effects();
    assert_eq!(effects.len(), 1);
    assert!(is_invalidate(&effects[0]));
    let view = controller.view();
    assert_eq!(view.phase, DeliveryPhase::Ready);
    assert_eq!(view.cursor, Some(ConversationCursor::new(4)));
    assert!(view.has_snapshot);
}

// 3. contiguous batch stays ready and emits one render invalidation
#[test]
fn contiguous_batch_stays_ready_and_invalidates() {
    let mut controller = ConversationDeliveryController::new(thread_id());
    let _ = controller.drain_effects();
    controller
        .on_snapshot(baseline_snapshot())
        .expect("baseline");
    let _ = controller.drain_effects();
    let batch_ok = batch(
        4,
        5,
        vec![lifecycle_patch(
            5,
            ITEM_USER,
            1,
            ConversationLifecycle::Completed,
        )],
    );
    controller
        .on_batch(batch_ok)
        .expect("contiguous batch applies");
    assert_eq!(controller.phase(), DeliveryPhase::Ready);
    assert_eq!(controller.cursor(), Some(ConversationCursor::new(5)));
    let effects = controller.drain_effects();
    assert_eq!(effects.len(), 1);
    assert!(is_invalidate(&effects[0]));
}

// 4. gap enters recovering, retains previous cursor/rows, emits report plus resnapshot request
#[test]
fn gap_enters_recovering_and_retains_state() {
    let mut controller = ConversationDeliveryController::new(thread_id());
    let _ = controller.drain_effects();
    controller
        .on_snapshot(baseline_snapshot())
        .expect("baseline");
    let _ = controller.drain_effects();
    // Gap: skip 5, start at 6
    let gap = batch(6, 7, vec![append_patch(7, ITEM_USER, 1, "x")]);
    controller.on_batch(gap).expect("gap handled");
    assert_eq!(controller.phase(), DeliveryPhase::Recovering);
    assert_eq!(
        controller.projection_status(),
        ProjectionStatus::ResnapshotRequired
    );
    // Last good snapshot retained
    assert_eq!(controller.cursor(), Some(ConversationCursor::new(4)));
    let effects = controller.drain_effects();
    assert_eq!(effects.len(), 2, "one report plus one resnapshot request");
    assert!(is_report(&effects[0]));
    assert!(is_request(&effects[1]));
    assert_eq!(request_generation(&effects[1]), Some(2));
    assert_eq!(
        request_after(&effects[1]),
        Some(Some(ConversationCursor::new(4)))
    );
}

// 5. more batches in recovering neither mutate visible state nor storm requests
#[test]
fn batches_in_recovering_do_not_mutate_or_storm() {
    let mut controller = ConversationDeliveryController::new(thread_id());
    let _ = controller.drain_effects();
    controller
        .on_snapshot(baseline_snapshot())
        .expect("baseline");
    let _ = controller.drain_effects();
    controller
        .on_batch(batch(6, 7, vec![append_patch(7, ITEM_USER, 1, "x")]))
        .expect("first gap");
    let _ = controller.drain_effects();
    let generation_after_recovery = controller.generation();
    assert_eq!(generation_after_recovery, 2);
    let retained_snapshot = controller.snapshot().cloned().expect("retained");

    // Two more invalid batches while recovering
    controller
        .on_batch(batch(4, 5, vec![append_patch(5, ITEM_USER, 1, "y")]))
        .expect("recovering batch");
    controller
        .on_batch(batch(4, 5, vec![append_patch(5, ITEM_USER, 1, "z")]))
        .expect("second recovering batch");

    assert_eq!(controller.phase(), DeliveryPhase::Recovering);
    assert_eq!(controller.snapshot(), Some(&retained_snapshot));
    assert_eq!(controller.cursor(), Some(ConversationCursor::new(4)));
    assert_eq!(
        controller.generation(),
        generation_after_recovery,
        "no storm"
    );

    let effects = controller.drain_effects();
    // Each batch should report but not create new snapshot requests
    assert_eq!(effects.len(), 2);
    assert!(effects.iter().all(is_report));
    assert!(!effects.iter().any(is_request));
    // No invalidation during recovering
    assert!(!effects.iter().any(is_invalidate));
}

// 6. explicit retry increments only generation and preserves recovery snapshot/cursor
#[test]
fn explicit_retry_increments_generation_and_preserves_recovery_snapshot() {
    let mut controller = ConversationDeliveryController::new(thread_id());
    let _ = controller.drain_effects();
    controller
        .on_snapshot(baseline_snapshot())
        .expect("baseline");
    let _ = controller.drain_effects();
    controller
        .on_batch(batch(6, 7, vec![append_patch(7, ITEM_USER, 1, "gap")]))
        .expect("enter recovering");
    let _ = controller.drain_effects();
    let before_snapshot = controller.snapshot().cloned().expect("recovery snapshot");
    let before_cursor = controller.cursor();
    let before_generation = controller.generation();

    controller.retry().expect("retry should succeed");
    assert_eq!(controller.generation(), before_generation + 1);
    assert_eq!(controller.snapshot(), Some(&before_snapshot));
    assert_eq!(controller.cursor(), before_cursor);
    assert_eq!(controller.phase(), DeliveryPhase::Recovering);

    let effects = controller.drain_effects();
    assert_eq!(effects.len(), 1);
    assert!(is_request(&effects[0]));
    assert_eq!(request_generation(&effects[0]), Some(before_generation + 1));
    assert_eq!(
        request_after(&effects[0]),
        Some(Some(ConversationCursor::new(4)))
    );
}

// 7. valid recovery snapshot returns ready
#[test]
fn valid_recovery_snapshot_returns_ready() {
    let mut controller = ConversationDeliveryController::new(thread_id());
    let _ = controller.drain_effects();
    controller
        .on_snapshot(baseline_snapshot())
        .expect("baseline");
    let _ = controller.drain_effects();
    controller
        .on_batch(batch(6, 7, vec![append_patch(7, ITEM_USER, 1, "gap")]))
        .expect("gap to recovering");
    let _ = controller.drain_effects();

    let recovery_snapshot = snapshot(
        7,
        vec![make_turn(TURN_A, 0, 0, ConversationLifecycle::Pending)],
        vec![make_assistant(ITEM_ASSIST, TURN_A, 2, "recovered")],
        40,
    );
    // Recovery must carry higher/equal cursor; 7 > 4 so projection will accept
    controller
        .on_snapshot(recovery_snapshot)
        .expect("recovery snapshot");

    assert_eq!(controller.phase(), DeliveryPhase::Ready);
    assert_eq!(controller.projection_status(), ProjectionStatus::Ready);
    assert_eq!(controller.cursor(), Some(ConversationCursor::new(7)));
    let effects = controller.drain_effects();
    assert_eq!(effects.len(), 1);
    assert!(is_invalidate(&effects[0]));
}

// 8. foreign frames do not alter phase or generation
#[test]
fn foreign_frames_do_not_alter_phase_or_generation() {
    let mut controller = ConversationDeliveryController::new(thread_id());
    let _ = controller.drain_effects();
    controller
        .on_snapshot(baseline_snapshot())
        .expect("baseline");
    let _ = controller.drain_effects();
    let before_phase = controller.phase();
    let before_generation = controller.generation();
    let before_snapshot = controller.snapshot().cloned();

    // Foreign snapshot
    controller
        .on_snapshot(foreign_snapshot())
        .expect("foreign snapshot handled");
    assert_eq!(controller.phase(), before_phase);
    assert_eq!(controller.generation(), before_generation);
    assert_eq!(controller.snapshot(), before_snapshot.as_ref());

    // Foreign batch
    controller
        .on_batch(foreign_batch(7, 8))
        .expect("foreign batch handled");
    assert_eq!(controller.phase(), before_phase);
    assert_eq!(controller.generation(), before_generation);
    assert_eq!(controller.snapshot(), before_snapshot.as_ref());

    let effects = controller.drain_effects();
    // Both foreign frames should have produced refusal reports but no new requests or invalidations
    assert_eq!(effects.len(), 2);
    assert!(effects.iter().all(is_report));
    assert!(!effects.iter().any(is_request));
    assert!(!effects.iter().any(is_invalidate));
}

// 9. close is terminal and idempotent
#[test]
fn close_is_terminal_and_idempotent() {
    let mut controller = ConversationDeliveryController::new(thread_id());
    let _ = controller.drain_effects();
    controller
        .on_snapshot(baseline_snapshot())
        .expect("baseline");
    let _ = controller.drain_effects();

    controller.close().expect("first close");
    let effects = controller.drain_effects();
    assert_eq!(effects.len(), 1);
    assert!(matches!(
        effects[0],
        ConversationDeliveryEffect::OwnerClosed { .. }
    ));
    assert_eq!(controller.phase(), DeliveryPhase::Closed);
    assert!(controller.is_closed());

    // Second close is idempotent — no second OwnerClosed and no error
    controller.close().expect("second close idempotent");
    let effects = controller.drain_effects();
    assert_eq!(effects.len(), 0, "second close emits nothing");

    // Later delivery is ignored — no state, projection, or generation change
    let gen_before = controller.generation();
    let snapshot_before = controller.snapshot().cloned();
    controller
        .on_snapshot(baseline_snapshot())
        .expect("snapshot after close ignored");
    controller
        .on_batch(batch(4, 5, vec![append_patch(5, ITEM_USER, 1, "x")]))
        .expect("batch after close ignored");
    assert_eq!(controller.phase(), DeliveryPhase::Closed);
    assert_eq!(controller.generation(), gen_before);
    assert_eq!(controller.snapshot(), snapshot_before.as_ref());
    let effects = controller.drain_effects();
    assert_eq!(
        effects.len(),
        0,
        "closed owner emits nothing for later delivery"
    );
    // View reports closed phase and pending count zero
    let view = controller.view();
    assert_eq!(view.phase, DeliveryPhase::Closed);
    assert_eq!(view.pending_effects, 0);
}

// 10. request-generation exhaustion is typed and never wraps
#[test]
fn generation_exhaustion_is_typed_and_never_wraps() {
    // Start two steps before exhaustion so we can observe one success then failure
    let mut controller =
        ConversationDeliveryController::with_initial_generation(thread_id(), u64::MAX - 2);
    // Construction consumed one generation: MAX-1
    // Drain that baseline request with generation MAX-1
    let init_effects = controller.drain_effects();
    assert_eq!(init_effects.len(), 1);
    assert_eq!(request_generation(&init_effects[0]), Some(u64::MAX - 1));

    // First need to enter recovering to allow a retry that allocates MAX
    // Install a baseline to go ready, then gap to enter recovering (which will allocate MAX)
    // But we already are at MAX-1, so we need a baseline: use normal snapshot
    controller
        .on_snapshot(baseline_snapshot())
        .expect("baseline should succeed even at high generation");
    let _ = controller.drain_effects();

    // Gap to trigger recovering -> this will try to allocate MAX (should succeed)
    controller
        .on_batch(batch(6, 7, vec![append_patch(7, ITEM_USER, 1, "gap")]))
        .expect("gap to recovering at MAX");
    let effects = controller.drain_effects();
    // Should have report + request with MAX
    assert!(effects.iter().any(is_report));
    let req = effects
        .iter()
        .find(|effect| is_request(*effect))
        .expect("request at MAX");
    assert_eq!(request_generation(req), Some(u64::MAX));
    assert_eq!(controller.generation(), u64::MAX);

    // Explicit retry must now exhaust: checked overflow
    let err = controller.retry().expect_err("retry at MAX should exhaust");
    assert_eq!(err, ConversationDeliveryError::GenerationExhausted);
    let effects = controller.drain_effects();
    assert!(effects
        .iter()
        .any(|e| matches!(e, ConversationDeliveryEffect::GenerationExhausted)));
    // Generation never wrapped
    assert_eq!(controller.generation(), u64::MAX);

    // Another retry keeps failing with same typed error and never wraps
    let err2 = controller
        .retry()
        .expect_err("second retry still exhausted");
    assert_eq!(err2, ConversationDeliveryError::GenerationExhausted);
    assert_eq!(controller.generation(), u64::MAX);
    let _ = controller.drain_effects();

    // Foreign frame should not increment generation either
    let generation_before = controller.generation();
    controller
        .on_snapshot(foreign_snapshot())
        .expect("foreign after exhaustion still handled");
    assert_eq!(controller.generation(), generation_before);
}

// Additional: refused snapshot that leaves status unchanged does not invent recovery
#[test]
fn refused_snapshot_does_not_invent_recovery() {
    let mut controller = ConversationDeliveryController::new(thread_id());
    let _ = controller.drain_effects();
    controller
        .on_snapshot(baseline_snapshot())
        .expect("baseline");
    let _ = controller.drain_effects();
    let before_phase = controller.phase();
    let before_generation = controller.generation();

    // Same cursor but conflicting rows -> SnapshotConflict, status unchanged
    let conflicting = snapshot(
        4,
        vec![make_turn(TURN_A, 0, 0, ConversationLifecycle::Pending)],
        vec![make_user(ITEM_USER, TURN_A, 1, "Edited")],
        30,
    );
    controller
        .on_snapshot(conflicting)
        .expect("conflicting snapshot handled");
    assert_eq!(controller.phase(), before_phase);
    assert_eq!(controller.generation(), before_generation);
    assert_eq!(controller.projection_status(), ProjectionStatus::Ready);
    let effects = controller.drain_effects();
    assert_eq!(effects.len(), 1);
    assert!(is_report(&effects[0]));
    assert!(!effects.iter().any(is_request));
}

// Additional: patch batches during recovering cannot emit invalidation
#[test]
fn recovering_batches_do_not_invalidate() {
    let mut controller = ConversationDeliveryController::new(thread_id());
    let _ = controller.drain_effects();
    controller
        .on_snapshot(baseline_snapshot())
        .expect("baseline");
    let _ = controller.drain_effects();
    controller
        .on_batch(batch(6, 7, vec![append_patch(7, ITEM_USER, 1, "gap")]))
        .expect("gap");
    let _ = controller.drain_effects();

    // Valid-looking contiguous batch would normally invalidate, but during
    // recovering it must be refused and must not emit invalidation
    controller
        .on_batch(batch(
            4,
            5,
            vec![lifecycle_patch(
                5,
                ITEM_USER,
                1,
                ConversationLifecycle::Completed,
            )],
        ))
        .expect("batch during recovering");
    let effects = controller.drain_effects();
    assert!(effects.iter().all(is_report));
    assert!(!effects.iter().any(is_invalidate));
    assert!(!effects.iter().any(is_request));
}

// Additional: view reports pending effect count without exposing mutable state
#[test]
fn view_reports_pending_count_without_mutable_exposure() {
    let mut controller = ConversationDeliveryController::new(thread_id());
    let view = controller.view();
    assert_eq!(view.pending_effects, 1);
    assert_eq!(view.phase, DeliveryPhase::AwaitingSnapshot);
    assert_eq!(view.thread_id, thread_id());
    assert_eq!(view.projection_status, ProjectionStatus::AwaitingSnapshot);
    assert!(view.cursor.is_none());
    assert!(!view.has_snapshot);
    // Draining changes count
    let _ = controller.drain_effects();
    assert_eq!(controller.view().pending_effects, 0);
    assert_eq!(controller.pending_effect_count(), 0);
}

#[test]
fn drain_does_not_alter_phase_or_projection() {
    let mut controller = ConversationDeliveryController::new(thread_id());
    let phase_before = controller.phase();
    let projection_before = controller.projection_status();
    let snapshot_before = controller.snapshot().cloned();
    let pending_before = controller.pending_effect_count();
    assert_eq!(pending_before, 1);

    let drained = controller.drain_effects();
    assert_eq!(drained.len(), 1);

    // Draining is a safe ownership move of the external context; it must not
    // mutate Statig shared storage.
    assert_eq!(controller.phase(), phase_before);
    assert_eq!(controller.projection_status(), projection_before);
    assert_eq!(controller.snapshot().cloned(), snapshot_before);
    assert_eq!(controller.pending_effect_count(), 0);
    assert_eq!(controller.view().pending_effects, 0);

    // Second drain is also inert
    let drained_again = controller.drain_effects();
    assert!(drained_again.is_empty());
    assert_eq!(controller.phase(), phase_before);
}

#[test]
fn undrained_exhaustion_not_rereported_on_later_inert_events() {
    let mut controller =
        ConversationDeliveryController::with_initial_generation(thread_id(), u64::MAX - 2);
    // Construction consumed one generation: MAX-1.
    let init_effects = controller.drain_effects();
    assert_eq!(init_effects.len(), 1);
    assert_eq!(request_generation(&init_effects[0]), Some(u64::MAX - 1));
    controller
        .on_snapshot(baseline_snapshot())
        .expect("baseline");
    let _ = controller.drain_effects();
    controller
        .on_batch(batch(6, 7, vec![append_patch(7, ITEM_USER, 1, "gap")]))
        .expect("gap allocates MAX");
    let _ = controller.drain_effects();
    assert_eq!(controller.generation(), u64::MAX);

    // Exhaust on retry — do not drain the resulting GenerationExhausted yet
    let err = controller.retry().expect_err("retry exhausts");
    assert_eq!(err, ConversationDeliveryError::GenerationExhausted);
    assert_eq!(controller.pending_effect_count(), 1);
    assert!(controller
        .pending_effects()
        .iter()
        .any(|e| matches!(e, ConversationDeliveryEffect::GenerationExhausted)));

    // An unrelated inert foreign frame must not rediscover the pending
    // exhaustion as a newly generated error.
    let result = controller.on_snapshot(foreign_snapshot());
    assert!(
        result.is_ok(),
        "undrained earlier exhaustion must not cause later inert dispatch to error"
    );
    // Foreign frame added its own report, so pending now has 2 effects
    assert_eq!(controller.pending_effect_count(), 2);
    assert!(controller.pending_effects()[1..]
        .iter()
        .all(|e| matches!(e, ConversationDeliveryEffect::ReportRefusal { .. })));

    // Closing after exhaustion must remain deterministic and not re-error
    // even though exhaustion is still pending.
    let close_result = controller.close();
    assert!(
        close_result.is_ok(),
        "close after undrained exhaustion must not rediscover error"
    );
    assert_eq!(controller.phase(), DeliveryPhase::Closed);
    assert!(controller.is_closed());

    // Only after draining does the pending count clear; no hidden re-error.
    let drained = controller.drain_effects();
    assert!(drained
        .iter()
        .any(|e| matches!(e, ConversationDeliveryEffect::GenerationExhausted)));
    assert_eq!(controller.pending_effect_count(), 0);
}
