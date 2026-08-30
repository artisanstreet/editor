//! Black-box tests for the pure synchronous steering placement machine.
//!
//! Every test uses only the public controller API via the registered crate
//! module path. No global singleton is assumed — concurrent controllers are
//! distinct. Effects live in an ordered drainable outbox; stale/mismatched
//! completions are atomic refusals.

use artisan_domain::{ItemId, RequestId};
use artisan_frontend::conversation_steering_machine::{
    ConversationSteeringMachine, SourceReference, SteeringEffect, SteeringEvent,
    SteeringFailureKind, SteeringLabelKind, SteeringPhase, SteeringPlacement,
};

fn item(id: &str) -> ItemId {
    ItemId::parse(id.to_owned()).expect("valid item id")
}

fn request(id: &str) -> RequestId {
    RequestId::parse(id.to_owned()).expect("valid request id")
}

fn source_ref(value: &str) -> SourceReference {
    SourceReference::parse(value.to_owned()).expect("valid source ref")
}

fn new_controller(
    command_id: &str,
    generation: u64,
    source_ref: &str,
    started_at: i64,
) -> ConversationSteeringMachine {
    ConversationSteeringMachine::new(
        command_id,
        generation,
        source_ref,
        started_at,
        SteeringLabelKind::Steering,
    )
    .expect("valid controller")
}

fn pending_lip_placement(placement: &SteeringPlacement) -> bool {
    matches!(placement, SteeringPlacement::ComposerPendingLip)
}

// ---------------------------------------------------------------------------
// 1. pending -> dispatching -> awaiting projection -> exact anchor ->
//    acknowledgement has the required view/effect sequence
// ---------------------------------------------------------------------------

#[test]
fn happy_path_view_and_effect_sequence() {
    let mut machine = new_controller("cmd-1", 1, "src-ref-1", 1000);

    // Initial: pending lip
    let view = machine.view();
    assert_eq!(view.phase, SteeringPhase::PendingLip);
    assert!(pending_lip_placement(&view.placement));
    assert_eq!(view.anchor, None);
    assert_eq!(view.pending_effect_count, 0);
    assert!(machine.drain_effects().is_empty());

    // DispatchStarted -> Dispatching
    machine
        .handle_event(SteeringEvent::DispatchStarted {
            command_id: request("cmd-1"),
            generation: 1,
            at_ms: 1001,
        })
        .expect("dispatch start");
    let view = machine.view();
    assert_eq!(view.phase, SteeringPhase::Dispatching);
    assert!(pending_lip_placement(&view.placement));
    let effects = machine.drain_effects();
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, SteeringEffect::Dispatch { .. }))
    );
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, SteeringEffect::WatchProjection { .. }))
    );
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, SteeringEffect::RenderInvalidation { .. }))
    );
    // Dispatch effect must carry bounded identity only, never raw prompt.
    for eff in &effects {
        if let SteeringEffect::Dispatch {
            source_reference, ..
        } = eff
        {
            assert_eq!(source_reference.as_str(), "src-ref-1");
        }
    }

    // DispatchAccepted -> AwaitingProjection (lip retained per invariant)
    machine
        .handle_event(SteeringEvent::DispatchAccepted {
            command_id: request("cmd-1"),
            generation: 1,
            at_ms: 1002,
        })
        .expect("accepted");
    let view = machine.view();
    assert_eq!(view.phase, SteeringPhase::AwaitingProjection);
    // Invariant: lip retained while awaiting projection.
    assert!(pending_lip_placement(&view.placement));
    assert_eq!(view.anchor, None);
    let effects = machine.drain_effects();
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, SteeringEffect::RenderInvalidation { .. }))
    );

    // DurableItemAnchored -> AwaitingAcknowledgement with exact anchor
    let anchor = item("item-aaaa");
    machine
        .handle_event(SteeringEvent::DurableItemAnchored {
            command_id: request("cmd-1"),
            generation: 1,
            item_id: anchor.clone(),
            at_ms: 1003,
        })
        .expect("anchored");
    let view = machine.view();
    assert_eq!(view.phase, SteeringPhase::AwaitingAcknowledgement);
    assert_eq!(view.anchor, Some(anchor.clone()));
    assert!(matches!(
        view.placement,
        SteeringPlacement::AnchoredAfter { anchor: ref a } if a == &anchor
    ));
    let effects = machine.drain_effects();
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, SteeringEffect::WatchAcknowledgement { .. }))
    );

    // EngineAcknowledged -> Acknowledged (settled, hidden, releases lip)
    machine
        .handle_event(SteeringEvent::EngineAcknowledged {
            command_id: request("cmd-1"),
            generation: 1,
            at_ms: 1004,
        })
        .expect("acknowledged");
    let view = machine.view();
    assert_eq!(view.phase, SteeringPhase::Acknowledged);
    assert_eq!(view.placement, SteeringPlacement::SettledHidden);
    // Anchor must still be exact, not moved or fabricated.
    assert_eq!(view.anchor, Some(anchor.clone()));
    assert_eq!(view.settled_at_ms, Some(1004));
    assert_eq!(view.started_at_ms, 1000);
    let effects = machine.drain_effects();
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, SteeringEffect::ReleasePendingLip { generation: 1 }))
    );
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, SteeringEffect::RenderInvalidation { .. }))
    );

    // View reports correct identity/routing data and effect count.
    let view = machine.view();
    assert_eq!(view.command_id, request("cmd-1"));
    assert_eq!(view.generation, 1);
    assert_eq!(view.source_reference, source_ref("src-ref-1"));
}

// ---------------------------------------------------------------------------
// 2. two concurrent controllers retain different exact anchors and labels
// ---------------------------------------------------------------------------

#[test]
fn two_concurrent_controllers_retain_different_anchors() {
    let mut a = new_controller("cmd-a", 1, "src-a", 2000);
    let mut b = new_controller("cmd-b", 7, "src-b", 2000);

    for (m, cmd) in [(&mut a, "cmd-a"), (&mut b, "cmd-b")] {
        m.handle_event(SteeringEvent::DispatchStarted {
            command_id: request(cmd),
            generation: if cmd == "cmd-a" { 1 } else { 7 },
            at_ms: 2001,
        })
        .unwrap();
        m.handle_event(SteeringEvent::DispatchAccepted {
            command_id: request(cmd),
            generation: if cmd == "cmd-a" { 1 } else { 7 },
            at_ms: 2002,
        })
        .unwrap();
    }

    let anchor_a = item("item-anchor-a");
    let anchor_b = item("item-anchor-b");
    a.handle_event(SteeringEvent::DurableItemAnchored {
        command_id: request("cmd-a"),
        generation: 1,
        item_id: anchor_a.clone(),
        at_ms: 2003,
    })
    .unwrap();
    b.handle_event(SteeringEvent::DurableItemAnchored {
        command_id: request("cmd-b"),
        generation: 7,
        item_id: anchor_b.clone(),
        at_ms: 2003,
    })
    .unwrap();

    assert_eq!(a.view().anchor, Some(anchor_a.clone()));
    assert_eq!(b.view().anchor, Some(anchor_b.clone()));
    assert_ne!(a.view().anchor, b.view().anchor);
    assert_ne!(a.view().command_id, b.view().command_id);
    assert_ne!(a.view().generation, b.view().generation);
    assert_ne!(a.view().source_reference, b.view().source_reference);

    assert!(matches!(
        a.view().placement,
        SteeringPlacement::AnchoredAfter { anchor: ref x } if x == &anchor_a
    ));
    assert!(matches!(
        b.view().placement,
        SteeringPlacement::AnchoredAfter { anchor: ref x } if x == &anchor_b
    ));

    // Settling one does not affect the other.
    a.handle_event(SteeringEvent::EngineAcknowledged {
        command_id: request("cmd-a"),
        generation: 1,
        at_ms: 2004,
    })
    .unwrap();
    assert_eq!(a.view().phase, SteeringPhase::Acknowledged);
    assert_eq!(b.view().phase, SteeringPhase::AwaitingAcknowledgement);
}

// ---------------------------------------------------------------------------
// 3. a later unrelated user item cannot relocate an anchored label
// ---------------------------------------------------------------------------

#[test]
fn later_unrelated_item_cannot_relocate_anchor() {
    let mut machine = new_controller("cmd-1", 1, "src-1", 3000);
    machine
        .handle_event(SteeringEvent::DispatchStarted {
            command_id: request("cmd-1"),
            generation: 1,
            at_ms: 3001,
        })
        .unwrap();
    machine
        .handle_event(SteeringEvent::DispatchAccepted {
            command_id: request("cmd-1"),
            generation: 1,
            at_ms: 3002,
        })
        .unwrap();
    let first = item("item-first");
    machine
        .handle_event(SteeringEvent::DurableItemAnchored {
            command_id: request("cmd-1"),
            generation: 1,
            item_id: first.clone(),
            at_ms: 3003,
        })
        .unwrap();
    // Attempt to anchor a different item — must be rejected.
    let second = item("item-second");
    let err = machine
        .handle_event(SteeringEvent::DurableItemAnchored {
            command_id: request("cmd-1"),
            generation: 1,
            item_id: second.clone(),
            at_ms: 3004,
        })
        .expect_err("conflict");
    assert!(matches!(
        err,
        artisan_frontend::conversation_steering_machine::SteeringRejection::AnchorConflict { .. }
    ));
    // View still shows first anchor.
    assert_eq!(machine.view().anchor, Some(first.clone()));
    assert!(matches!(
        machine.view().placement,
        SteeringPlacement::AnchoredAfter { anchor: ref a } if a == &first
    ));

    // Even after acknowledgement, the anchor must not move.
    machine
        .handle_event(SteeringEvent::EngineAcknowledged {
            command_id: request("cmd-1"),
            generation: 1,
            at_ms: 3005,
        })
        .unwrap();
    // Settled — anchoring a new item is refused, not relocated.
    let after_settled = machine.handle_event(SteeringEvent::DurableItemAnchored {
        command_id: request("cmd-1"),
        generation: 1,
        item_id: second.clone(),
        at_ms: 3006,
    });
    assert!(after_settled.is_err());
    assert_eq!(machine.view().anchor, Some(first));
}

// ---------------------------------------------------------------------------
// 4. stale command/generation completions are atomic refusals
// ---------------------------------------------------------------------------

#[test]
fn stale_completions_are_atomic_refusals() {
    let mut machine = new_controller("cmd-1", 5, "src-1", 4000);
    machine
        .handle_event(SteeringEvent::DispatchStarted {
            command_id: request("cmd-1"),
            generation: 5,
            at_ms: 4001,
        })
        .unwrap();
    machine
        .handle_event(SteeringEvent::DispatchAccepted {
            command_id: request("cmd-1"),
            generation: 5,
            at_ms: 4002,
        })
        .unwrap();
    // Drain so we can detect if stale event incorrectly drains newer effects.
    let _ = machine.drain_effects();
    let view_before = machine.view();
    let effects_before = machine.pending_effects();

    // Stale generation.
    let err = machine
        .handle_event(SteeringEvent::EngineAcknowledged {
            command_id: request("cmd-1"),
            generation: 4,
            at_ms: 4003,
        })
        .expect_err("stale generation");
    assert!(matches!(
        err,
        artisan_frontend::conversation_steering_machine::SteeringRejection::StaleCommandOrGeneration { .. }
    ));
    assert_eq!(machine.view(), view_before);
    assert_eq!(machine.pending_effects(), effects_before);

    // Stale command id.
    let err2 = machine
        .handle_event(SteeringEvent::EngineAcknowledged {
            command_id: request("cmd-999"),
            generation: 5,
            at_ms: 4003,
        })
        .expect_err("stale command");
    assert!(matches!(
        err2,
        artisan_frontend::conversation_steering_machine::SteeringRejection::StaleCommandOrGeneration { .. }
    ));
    assert_eq!(machine.view(), view_before);
    assert_eq!(machine.pending_effects(), effects_before);

    // Stale anchor event.
    let err3 = machine
        .handle_event(SteeringEvent::DurableItemAnchored {
            command_id: request("cmd-1"),
            generation: 99,
            item_id: item("item-x"),
            at_ms: 4003,
        })
        .expect_err("stale gen anchor");
    assert!(matches!(
        err3,
        artisan_frontend::conversation_steering_machine::SteeringRejection::StaleCommandOrGeneration { .. }
    ));
    assert_eq!(machine.view(), view_before);
}

// ---------------------------------------------------------------------------
// 5. duplicate equal anchor/completion is idempotent while conflicting anchor
//    is rejected
// ---------------------------------------------------------------------------

#[test]
fn duplicate_equal_anchor_is_idempotent_conflicting_rejected() {
    let mut machine = new_controller("cmd-1", 1, "src-1", 5000);
    machine
        .handle_event(SteeringEvent::DispatchStarted {
            command_id: request("cmd-1"),
            generation: 1,
            at_ms: 5001,
        })
        .unwrap();
    machine
        .handle_event(SteeringEvent::DispatchAccepted {
            command_id: request("cmd-1"),
            generation: 1,
            at_ms: 5002,
        })
        .unwrap();
    let anchor = item("item-anchor");
    machine
        .handle_event(SteeringEvent::DurableItemAnchored {
            command_id: request("cmd-1"),
            generation: 1,
            item_id: anchor.clone(),
            at_ms: 5003,
        })
        .unwrap();
    let view_after_first = machine.view();
    let effects_after_first = machine.pending_effects();
    // Duplicate equal anchor with newer timestamp — idempotent, no conflict,
    // advances last_observed but does not change placement or duplicate effects.
    machine
        .handle_event(SteeringEvent::DurableItemAnchored {
            command_id: request("cmd-1"),
            generation: 1,
            item_id: anchor.clone(),
            at_ms: 5004,
        })
        .expect("duplicate equal idempotent");
    assert_eq!(machine.view().anchor, Some(anchor.clone()));
    assert_eq!(machine.view().phase, view_after_first.phase);
    let watch_count = machine
        .pending_effects()
        .iter()
        .filter(|e| matches!(e, SteeringEffect::WatchAcknowledgement { .. }))
        .count();
    let watch_count_before = effects_after_first
        .iter()
        .filter(|e| matches!(e, SteeringEffect::WatchAcknowledgement { .. }))
        .count();
    assert_eq!(watch_count, watch_count_before);

    // Conflicting anchor is rejected.
    let other = item("item-other");
    let err = machine
        .handle_event(SteeringEvent::DurableItemAnchored {
            command_id: request("cmd-1"),
            generation: 1,
            item_id: other,
            at_ms: 5005,
        })
        .expect_err("conflict");
    assert!(matches!(
        err,
        artisan_frontend::conversation_steering_machine::SteeringRejection::AnchorConflict { .. }
    ));

    // Duplicate acknowledgement is idempotent when settled.
    machine
        .handle_event(SteeringEvent::EngineAcknowledged {
            command_id: request("cmd-1"),
            generation: 1,
            at_ms: 5006,
        })
        .unwrap();
    let settled_view = machine.view();
    machine
        .handle_event(SteeringEvent::EngineAcknowledged {
            command_id: request("cmd-1"),
            generation: 1,
            at_ms: 5007,
        })
        .expect("duplicate ack idempotent");
    assert_eq!(machine.view(), settled_view);
}

// ---------------------------------------------------------------------------
// 6. dispatch failure/cancellation release only the owning generation
// ---------------------------------------------------------------------------

#[test]
fn failure_and_cancellation_release_only_owning_generation() {
    let mut a = new_controller("cmd-a", 10, "src-a", 6000);
    let mut b = new_controller("cmd-b", 20, "src-b", 6000);
    for (m, cmd, generation) in [(&mut a, "cmd-a", 10), (&mut b, "cmd-b", 20)] {
        m.handle_event(SteeringEvent::DispatchStarted {
            command_id: request(cmd),
            generation,
            at_ms: 6001,
        })
        .unwrap();
    }
    // Drain dispatch effects.
    let _ = a.drain_effects();
    let _ = b.drain_effects();

    // Fail A with closed redacted kind
    a.handle_event(SteeringEvent::DispatchFailed {
        command_id: request("cmd-a"),
        generation: 10,
        at_ms: 6002,
        kind: SteeringFailureKind::Dispatch,
    })
    .unwrap();
    let effects_a = a.drain_effects();
    assert!(
        effects_a
            .iter()
            .any(|e| matches!(e, SteeringEffect::ReleasePendingLip { generation: 10 }))
    );
    assert!(
        !effects_a
            .iter()
            .any(|e| matches!(e, SteeringEffect::ReleasePendingLip { generation: 20 }))
    );
    assert_eq!(a.view().phase, SteeringPhase::Failed);
    assert!(matches!(
        a.view().placement,
        SteeringPlacement::Failed {
            kind: SteeringFailureKind::Dispatch
        }
    ));

    // B still dispatching — not affected.
    assert_eq!(b.view().phase, SteeringPhase::Dispatching);
    assert!(b.pending_effects().is_empty());

    // Cancel B
    b.handle_event(SteeringEvent::Cancelled {
        command_id: request("cmd-b"),
        generation: 20,
        at_ms: 6003,
    })
    .unwrap();
    let effects_b = b.drain_effects();
    assert!(
        effects_b
            .iter()
            .any(|e| matches!(e, SteeringEffect::ReleasePendingLip { generation: 20 }))
    );
    assert_eq!(b.view().phase, SteeringPhase::Cancelled);
    assert_eq!(b.view().placement, SteeringPlacement::SettledHidden);

    // Failure after anchoring must keep anchor and use redacted kind.
    let mut c = new_controller("cmd-c", 30, "src-c", 7000);
    c.handle_event(SteeringEvent::DispatchStarted {
        command_id: request("cmd-c"),
        generation: 30,
        at_ms: 7001,
    })
    .unwrap();
    c.handle_event(SteeringEvent::DispatchAccepted {
        command_id: request("cmd-c"),
        generation: 30,
        at_ms: 7002,
    })
    .unwrap();
    let anchor = item("item-c");
    c.handle_event(SteeringEvent::DurableItemAnchored {
        command_id: request("cmd-c"),
        generation: 30,
        item_id: anchor.clone(),
        at_ms: 7003,
    })
    .unwrap();
    c.handle_event(SteeringEvent::DispatchFailed {
        command_id: request("cmd-c"),
        generation: 30,
        at_ms: 7004,
        kind: SteeringFailureKind::Rejected,
    })
    .unwrap();
    assert_eq!(c.view().anchor, Some(anchor));
    assert_eq!(c.view().phase, SteeringPhase::Failed);
    assert!(matches!(
        c.view().placement,
        SteeringPlacement::Failed {
            kind: SteeringFailureKind::Rejected
        }
    ));
}

// ---------------------------------------------------------------------------
// 7. acknowledgement before projection does not fabricate placement
// ---------------------------------------------------------------------------

#[test]
fn acknowledgement_before_projection_settles_without_fabricating_anchor() {
    let mut machine = new_controller("cmd-1", 1, "src-1", 8000);
    machine
        .handle_event(SteeringEvent::DispatchStarted {
            command_id: request("cmd-1"),
            generation: 1,
            at_ms: 8001,
        })
        .unwrap();
    machine
        .handle_event(SteeringEvent::DispatchAccepted {
            command_id: request("cmd-1"),
            generation: 1,
            at_ms: 8002,
        })
        .unwrap();
    // Ack arrives before any anchor.
    machine
        .handle_event(SteeringEvent::EngineAcknowledged {
            command_id: request("cmd-1"),
            generation: 1,
            at_ms: 8003,
        })
        .unwrap();
    let view = machine.view();
    assert_eq!(view.phase, SteeringPhase::Acknowledged);
    assert_eq!(view.anchor, None);
    // Must be hidden, not AnchoredAfter with fabricated id.
    assert_eq!(view.placement, SteeringPlacement::SettledHidden);
    assert!(!matches!(
        view.placement,
        SteeringPlacement::AnchoredAfter { .. }
    ));
    // Later anchor attempt must be refused (settled, cannot fabricate).
    let anchor = item("item-late");
    let err = machine.handle_event(SteeringEvent::DurableItemAnchored {
        command_id: request("cmd-1"),
        generation: 1,
        item_id: anchor,
        at_ms: 8004,
    });
    assert!(err.is_err());
    assert_eq!(machine.view().anchor, None);
}

// ---------------------------------------------------------------------------
// 8. timestamp regression is refused without changing prior view/outbox
// ---------------------------------------------------------------------------

#[test]
fn timestamp_regression_refused_atomically() {
    let mut machine = new_controller("cmd-1", 1, "src-1", 9000);
    machine
        .handle_event(SteeringEvent::DispatchStarted {
            command_id: request("cmd-1"),
            generation: 1,
            at_ms: 9001,
        })
        .unwrap();
    machine
        .handle_event(SteeringEvent::DispatchAccepted {
            command_id: request("cmd-1"),
            generation: 1,
            at_ms: 9005,
        })
        .unwrap();
    let _ = machine.drain_effects();
    let view_before = machine.view();
    let effects_before = machine.pending_effects();

    // Regression: at_ms 9003 < last_observed 9005
    let err = machine
        .handle_event(SteeringEvent::DurableItemAnchored {
            command_id: request("cmd-1"),
            generation: 1,
            item_id: item("item-x"),
            at_ms: 9003,
        })
        .expect_err("regression");
    assert!(matches!(
        err,
        artisan_frontend::conversation_steering_machine::SteeringRejection::TimestampRegression { .. }
    ));
    assert_eq!(machine.view(), view_before);
    assert_eq!(machine.pending_effects(), effects_before);

    // Also for acknowledgement with regression.
    let err2 = machine
        .handle_event(SteeringEvent::EngineAcknowledged {
            command_id: request("cmd-1"),
            generation: 1,
            at_ms: 9004,
        })
        .expect_err("regression ack");
    assert!(matches!(
        err2,
        artisan_frontend::conversation_steering_machine::SteeringRejection::TimestampRegression { .. }
    ));
    assert_eq!(machine.view(), view_before);

    // Valid forward timestamp still succeeds.
    machine
        .handle_event(SteeringEvent::DurableItemAnchored {
            command_id: request("cmd-1"),
            generation: 1,
            item_id: item("item-y"),
            at_ms: 9006,
        })
        .expect("forward anchor");
    assert_eq!(machine.view().anchor, Some(item("item-y")));
}

// ---------------------------------------------------------------------------
// Additional: construction validation, bounded source ref, failure kind
// ---------------------------------------------------------------------------

#[test]
fn construction_rejects_invalid_id_generation_and_source_ref() {
    assert!(
        ConversationSteeringMachine::new("", 1, "src", 0, SteeringLabelKind::Steering).is_err()
    );
    assert!(
        ConversationSteeringMachine::new(
            "cmd with space",
            1,
            "src",
            0,
            SteeringLabelKind::Steering
        )
        .is_err()
    );
    assert!(
        ConversationSteeringMachine::new("cmd-1", 0, "src", 0, SteeringLabelKind::Steering)
            .is_err()
    );
    assert!(
        ConversationSteeringMachine::new("cmd-1", 1, "", 0, SteeringLabelKind::Steering).is_err()
    );
    // Bounded source reference: whitespace/control rejected
    assert!(SourceReference::parse("bad ref with space").is_err());
    assert!(SourceReference::parse("").is_err());
    // Too long ( >128 bytes)
    let long = "a".repeat(129);
    assert!(SourceReference::parse(long).is_err());
    assert!(
        ConversationSteeringMachine::new(
            "cmd-1",
            1,
            "bad ref with space",
            0,
            SteeringLabelKind::Steering
        )
        .is_err()
    );
}

#[test]
fn view_reflects_pending_effect_count_and_identity() {
    let mut m = new_controller("cmd-xyz", 42, "src-xyz", 12345);
    assert_eq!(m.view().pending_effect_count, 0);
    m.handle_event(SteeringEvent::DispatchStarted {
        command_id: request("cmd-xyz"),
        generation: 42,
        at_ms: 12346,
    })
    .unwrap();
    assert_eq!(m.view().pending_effect_count, 3);
    assert_eq!(m.view().command_id, request("cmd-xyz"));
    assert_eq!(m.view().generation, 42);
    assert_eq!(m.view().source_reference, source_ref("src-xyz"));
    assert_eq!(m.view().started_at_ms, 12345);
}

// ---------------------------------------------------------------------------
// 9. Statig is the source of truth — phase/placement derives from machine.state()
// ---------------------------------------------------------------------------

#[test]
fn statig_state_drives_phase_and_placement() {
    let mut m = new_controller("cmd-s", 1, "src-s", 100);
    // Initial pending lip
    assert_eq!(m.phase(), SteeringPhase::PendingLip);
    assert_eq!(m.view().phase, SteeringPhase::PendingLip);

    m.handle_event(SteeringEvent::DispatchStarted {
        command_id: request("cmd-s"),
        generation: 1,
        at_ms: 101,
    })
    .unwrap();
    assert_eq!(m.phase(), SteeringPhase::Dispatching);
    assert_eq!(m.view().phase, SteeringPhase::Dispatching);

    m.handle_event(SteeringEvent::DispatchAccepted {
        command_id: request("cmd-s"),
        generation: 1,
        at_ms: 102,
    })
    .unwrap();
    assert_eq!(m.phase(), SteeringPhase::AwaitingProjection);

    let anchor = item("item-s");
    m.handle_event(SteeringEvent::DurableItemAnchored {
        command_id: request("cmd-s"),
        generation: 1,
        item_id: anchor.clone(),
        at_ms: 103,
    })
    .unwrap();
    assert_eq!(m.phase(), SteeringPhase::AwaitingAcknowledgement);
    assert!(matches!(
        m.view().placement,
        SteeringPlacement::AnchoredAfter { anchor: ref a } if a == &anchor
    ));

    m.handle_event(SteeringEvent::EngineAcknowledged {
        command_id: request("cmd-s"),
        generation: 1,
        at_ms: 104,
    })
    .unwrap();
    assert_eq!(m.phase(), SteeringPhase::Acknowledged);
    assert_eq!(m.view().phase, SteeringPhase::Acknowledged);
    assert_eq!(m.view().placement, SteeringPlacement::SettledHidden);

    // Sealed settled: further dispatch is refused, proving Statig sealed superstate.
    let err = m.handle_event(SteeringEvent::DispatchStarted {
        command_id: request("cmd-s"),
        generation: 1,
        at_ms: 105,
    });
    assert!(matches!(
        err,
        Err(
            artisan_frontend::conversation_steering_machine::SteeringRejection::AlreadySettled { .. }
        )
    ));
    assert_eq!(m.phase(), SteeringPhase::Acknowledged);
}

// ---------------------------------------------------------------------------
// 10. Settled states are sealed — no retry or re-dispatch out of Failed/Cancelled
// ---------------------------------------------------------------------------

#[test]
fn settled_states_are_sealed_no_retry() {
    // Failed is sealed
    let mut m = new_controller("cmd-f", 1, "src-f", 200);
    m.handle_event(SteeringEvent::DispatchStarted {
        command_id: request("cmd-f"),
        generation: 1,
        at_ms: 201,
    })
    .unwrap();
    m.handle_event(SteeringEvent::DispatchFailed {
        command_id: request("cmd-f"),
        generation: 1,
        at_ms: 202,
        kind: SteeringFailureKind::Dispatch,
    })
    .unwrap();
    assert_eq!(m.phase(), SteeringPhase::Failed);
    // No retry transition — any further dispatch attempt is sealed.
    let err = m.handle_event(SteeringEvent::DispatchStarted {
        command_id: request("cmd-f"),
        generation: 1,
        at_ms: 203,
    });
    assert!(matches!(
        err,
        Err(
            artisan_frontend::conversation_steering_machine::SteeringRejection::AlreadySettled { .. }
        )
    ));
    // Duplicate DispatchFailed is idempotent
    m.handle_event(SteeringEvent::DispatchFailed {
        command_id: request("cmd-f"),
        generation: 1,
        at_ms: 204,
        kind: SteeringFailureKind::Dispatch,
    })
    .expect("duplicate failed idempotent");
    assert_eq!(m.phase(), SteeringPhase::Failed);

    // Cancelled is sealed
    let mut n = new_controller("cmd-c", 1, "src-c", 300);
    n.handle_event(SteeringEvent::DispatchStarted {
        command_id: request("cmd-c"),
        generation: 1,
        at_ms: 301,
    })
    .unwrap();
    n.handle_event(SteeringEvent::Cancelled {
        command_id: request("cmd-c"),
        generation: 1,
        at_ms: 302,
    })
    .unwrap();
    assert_eq!(n.phase(), SteeringPhase::Cancelled);
    let err2 = n.handle_event(SteeringEvent::DispatchAccepted {
        command_id: request("cmd-c"),
        generation: 1,
        at_ms: 303,
    });
    assert!(err2.is_err());
    assert_eq!(n.phase(), SteeringPhase::Cancelled);
}

// ---------------------------------------------------------------------------
// 11. Failure kind is a closed redacted enum — no raw strings in view/effects
// ---------------------------------------------------------------------------

#[test]
fn failure_kind_is_closed_and_redacted() {
    let mut m = new_controller("cmd-k", 1, "src-k", 400);
    m.handle_event(SteeringEvent::DispatchStarted {
        command_id: request("cmd-k"),
        generation: 1,
        at_ms: 401,
    })
    .unwrap();
    m.handle_event(SteeringEvent::DispatchFailed {
        command_id: request("cmd-k"),
        generation: 1,
        at_ms: 402,
        kind: SteeringFailureKind::Rejected,
    })
    .unwrap();
    assert!(matches!(
        m.view().placement,
        SteeringPlacement::Failed {
            kind: SteeringFailureKind::Rejected
        }
    ));
    // View Debug must not contain raw prompt text — only the redacted kind.
    let debug = format!("{:?}", m.view().placement);
    assert!(debug.contains("Rejected"));
    assert!(!debug.contains("prompt"));
}
