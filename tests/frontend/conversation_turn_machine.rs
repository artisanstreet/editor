//! Black-box coverage for the synchronous conversation turn state machine.

#[path = "../../modules/frontend/src/conversation_turn_machine.rs"]
mod conversation_turn_machine;

use conversation_turn_machine::{
    CompletionKind, ConversationTurnController, FailureKind, StateKind, TurnError, TurnEvent,
    TurnNarration,
};

fn assert_single_narration(controller: &ConversationTurnController) {
    let view = controller.view();
    // The narration enum makes two simultaneous labels unrepresentable; this
    // helper asserts we always have exactly one variant.
    match view.narration {
        TurnNarration::Hidden
        | TurnNarration::WaitingForProvider
        | TurnNarration::Compacting
        | TurnNarration::Thinking
        | TurnNarration::Working
        | TurnNarration::StreamingReply
        | TurnNarration::WaitingForBackground
        | TurnNarration::WorkedFor { .. }
        | TurnNarration::ThoughtFor { .. }
        | TurnNarration::Failed { .. }
        | TurnNarration::Interrupted { .. }
        | TurnNarration::Cancelled { .. } => {}
    }
}

#[test]
fn pending_through_provider_thinking_streaming_completes_as_thought() {
    let mut ctl = ConversationTurnController::new();
    assert_eq!(ctl.view().narration, TurnNarration::Hidden);
    assert_single_narration(&ctl);

    ctl.dispatch(TurnEvent::WaitingForProvider {
        at: 1_000,
        revision: 1,
    })
    .unwrap();
    assert_eq!(ctl.view().narration, TurnNarration::WaitingForProvider);
    assert_single_narration(&ctl);

    ctl.dispatch(TurnEvent::Thinking {
        at: 1_100,
        revision: 2,
    })
    .unwrap();
    assert_eq!(ctl.view().narration, TurnNarration::Thinking);
    assert!(ctl.reasoning_seen());
    assert!(!ctl.work_seen());
    assert_single_narration(&ctl);

    ctl.dispatch(TurnEvent::StreamingReply {
        at: 1_200,
        revision: 3,
    })
    .unwrap();
    assert_eq!(ctl.view().narration, TurnNarration::StreamingReply);
    // Streaming suppresses quiet status but preserves evidence.
    assert!(ctl.reasoning_seen());
    assert_single_narration(&ctl);

    ctl.dispatch(TurnEvent::Completed {
        at: 1_500,
        revision: 4,
    })
    .unwrap();
    let view = ctl.view();
    assert_eq!(view.state, StateKind::Completed);
    assert_eq!(view.started_at, Some(1_000));
    assert_eq!(view.terminal_at, Some(1_500));
    match view.narration {
        TurnNarration::ThoughtFor { elapsed_ms } => assert_eq!(elapsed_ms, 500),
        other => panic!("expected ThoughtFor, got {other:?}"),
    }
    assert_single_narration(&ctl);
}

#[test]
fn work_evidence_produces_worked_for_even_after_streaming() {
    let mut ctl = ConversationTurnController::new();

    ctl.dispatch(TurnEvent::Working {
        at: 2_000,
        revision: 1,
    })
    .unwrap();
    assert!(ctl.work_seen());
    assert_eq!(ctl.view().narration, TurnNarration::Working);

    ctl.dispatch(TurnEvent::StreamingReply {
        at: 2_100,
        revision: 2,
    })
    .unwrap();
    assert_eq!(ctl.view().narration, TurnNarration::StreamingReply);
    // Evidence not erased.
    assert!(ctl.work_seen());

    ctl.dispatch(TurnEvent::Completed {
        at: 2_600,
        revision: 3,
    })
    .unwrap();
    match ctl.view().narration {
        TurnNarration::WorkedFor { elapsed_ms } => assert_eq!(elapsed_ms, 600),
        other => panic!("expected WorkedFor, got {other:?}"),
    }

    // Work outranks thought when both seen.
    let mut ctl2 = ConversationTurnController::new();
    ctl2.dispatch(TurnEvent::Thinking {
        at: 3_000,
        revision: 1,
    })
    .unwrap();
    ctl2.dispatch(TurnEvent::Working {
        at: 3_100,
        revision: 2,
    })
    .unwrap();
    ctl2.dispatch(TurnEvent::StreamingReply {
        at: 3_200,
        revision: 3,
    })
    .unwrap();
    ctl2.dispatch(TurnEvent::Completed {
        at: 3_800,
        revision: 4,
    })
    .unwrap();
    match ctl2.view().narration {
        TurnNarration::WorkedFor { elapsed_ms } => assert_eq!(elapsed_ms, 800),
        other => panic!("expected WorkedFor when both seen, got {other:?}"),
    }
}

#[test]
fn compaction_and_background_wait_have_exact_distinct_narration() {
    let mut ctl = ConversationTurnController::new();
    ctl.dispatch(TurnEvent::Compacting {
        at: 10_000,
        revision: 1,
    })
    .unwrap();
    assert_eq!(ctl.view().narration, TurnNarration::Compacting);
    assert_eq!(ctl.view().narration.label(), "Compacting the conversation…");
    assert_single_narration(&ctl);

    // Compaction outranks generic thinking/working while current: staying in
    // Compacting leaf, not falling back to Thinking.
    ctl.dispatch(TurnEvent::Compacting {
        at: 10_100,
        revision: 2,
    })
    .unwrap();
    assert_eq!(ctl.view().narration, TurnNarration::Compacting);

    let mut ctl2 = ConversationTurnController::new();
    ctl2.dispatch(TurnEvent::WaitingForBackground {
        at: 20_000,
        revision: 1,
    })
    .unwrap();
    assert_eq!(ctl2.view().narration, TurnNarration::WaitingForBackground);
    assert_ne!(
        ctl.view().narration,
        ctl2.view().narration,
        "compacting and background waiting must be distinct variants"
    );
    assert_single_narration(&ctl2);

    // Background waiting is distinct from generic Working.
    let mut ctl3 = ConversationTurnController::new();
    ctl3.dispatch(TurnEvent::Working {
        at: 30_000,
        revision: 1,
    })
    .unwrap();
    assert_eq!(ctl3.view().narration, TurnNarration::Working);
    ctl3.dispatch(TurnEvent::WaitingForBackground {
        at: 30_100,
        revision: 2,
    })
    .unwrap();
    assert_eq!(ctl3.view().narration, TurnNarration::WaitingForBackground);
}

#[test]
fn streaming_suppresses_quiet_label_without_erasing_evidence() {
    let mut ctl = ConversationTurnController::new();
    ctl.dispatch(TurnEvent::Thinking {
        at: 5_000,
        revision: 1,
    })
    .unwrap();
    assert_eq!(ctl.view().narration, TurnNarration::Thinking);
    ctl.dispatch(TurnEvent::Working {
        at: 5_100,
        revision: 2,
    })
    .unwrap();
    assert_eq!(ctl.view().narration, TurnNarration::Working);
    assert!(ctl.reasoning_seen());
    assert!(ctl.work_seen());

    ctl.dispatch(TurnEvent::StreamingReply {
        at: 5_200,
        revision: 3,
    })
    .unwrap();
    assert_eq!(ctl.view().narration, TurnNarration::StreamingReply);
    // Quiet labels hidden while streaming, but evidence preserved for terminal.
    assert!(ctl.reasoning_seen());
    assert!(ctl.work_seen());

    ctl.dispatch(TurnEvent::Completed {
        at: 5_900,
        revision: 4,
    })
    .unwrap();
    match ctl.view().narration {
        TurnNarration::WorkedFor { elapsed_ms } => assert_eq!(elapsed_ms, 900),
        other => panic!("expected WorkedFor preserved after streaming, got {other:?}"),
    }
}

#[test]
fn interrupted_resume_preserves_original_duration_origin() {
    let mut ctl = ConversationTurnController::new();
    ctl.dispatch(TurnEvent::Working {
        at: 100_000,
        revision: 1,
    })
    .unwrap();
    assert_eq!(ctl.started_at(), Some(100_000));
    ctl.dispatch(TurnEvent::Interrupted {
        at: 100_500,
        revision: 2,
    })
    .unwrap();
    assert_eq!(ctl.state(), StateKind::Interrupted);
    assert_eq!(ctl.terminal_at(), Some(100_500));
    let interrupted_view = ctl.view();
    match interrupted_view.narration {
        TurnNarration::Interrupted { elapsed_ms } => assert_eq!(elapsed_ms, 500),
        other => panic!("expected Interrupted, got {other:?}"),
    }

    // Explicit resume without losing original start.
    ctl.dispatch(TurnEvent::Resume {
        at: 101_000,
        revision: 3,
    })
    .unwrap();
    assert_eq!(ctl.started_at(), Some(100_000));
    assert_eq!(ctl.phase_started_at(), Some(101_000));
    // Reasoning/work evidence preserved.
    assert!(ctl.work_seen());
    assert_eq!(ctl.state(), StateKind::Working);

    ctl.dispatch(TurnEvent::Completed {
        at: 102_000,
        revision: 4,
    })
    .unwrap();
    let view = ctl.view();
    assert_eq!(view.started_at, Some(100_000));
    match view.narration {
        TurnNarration::WorkedFor { elapsed_ms } => assert_eq!(elapsed_ms, 2_000),
        other => panic!("expected WorkedFor with preserved origin, got {other:?}"),
    }

    // Also resumable via direct active event after Interrupted.
    let mut ctl2 = ConversationTurnController::new();
    ctl2.dispatch(TurnEvent::Thinking {
        at: 200_000,
        revision: 1,
    })
    .unwrap();
    ctl2.dispatch(TurnEvent::Interrupted {
        at: 200_300,
        revision: 2,
    })
    .unwrap();
    ctl2.dispatch(TurnEvent::Thinking {
        at: 201_000,
        revision: 3,
    })
    .unwrap();
    assert_eq!(ctl2.started_at(), Some(200_000));
    ctl2.dispatch(TurnEvent::Completed {
        at: 202_000,
        revision: 4,
    })
    .unwrap();
    match ctl2.view().narration {
        TurnNarration::ThoughtFor { elapsed_ms } => assert_eq!(elapsed_ms, 2_000),
        other => panic!("expected ThoughtFor, got {other:?}"),
    }
}

#[test]
fn sealed_failed_and_cancelled_reject_different_events_duplicate_is_idempotent() {
    // Failed sealed.
    let mut ctl = ConversationTurnController::new();
    ctl.dispatch(TurnEvent::Working {
        at: 10_000,
        revision: 1,
    })
    .unwrap();
    ctl.dispatch(TurnEvent::Failed {
        at: 11_000,
        revision: 2,
        kind: Some(FailureKind::Timeout),
    })
    .unwrap();
    let failed_view = ctl.view();
    assert_eq!(failed_view.state, StateKind::Failed);

    // Duplicate exact settlement is harmless (idempotent) even with same revision.
    let dup = ctl.dispatch(TurnEvent::Failed {
        at: 11_000,
        revision: 2,
        kind: Some(FailureKind::Timeout),
    });
    assert!(dup.is_ok(), "exact duplicate settlement must be idempotent");
    assert_eq!(ctl.view(), failed_view);

    // Duplicate with higher revision also idempotent (monotonic forward).
    let dup2 = ctl.dispatch(TurnEvent::Failed {
        at: 11_000,
        revision: 3,
        kind: Some(FailureKind::Timeout),
    });
    // Our implementation treats same at/kind with higher revision as
    // duplicate idempotent (advances revision).
    assert!(dup2.is_ok());
    assert_eq!(ctl.view().terminal_at, Some(11_000));

    // Different later work/terminal is rejected without changing data.
    let before = ctl.view();
    let err = ctl
        .dispatch(TurnEvent::Working {
            at: 12_000,
            revision: 4,
        })
        .unwrap_err();
    assert!(matches!(err, TurnError::Sealed { .. }));
    assert_eq!(ctl.view(), before);

    let err2 = ctl
        .dispatch(TurnEvent::Completed {
            at: 12_000,
            revision: 5,
        })
        .unwrap_err();
    assert!(matches!(err2, TurnError::Sealed { .. }));
    assert_eq!(ctl.view(), before);

    // Different failure kind is not duplicate -> sealed rejection.
    let err3 = ctl.dispatch(TurnEvent::Failed {
        at: 11_000,
        revision: 6,
        kind: Some(FailureKind::Generic),
    });
    assert!(matches!(err3, Err(TurnError::Sealed { .. })));
    assert_eq!(ctl.view(), before);

    // Cancelled sealed similarly.
    let mut ctl2 = ConversationTurnController::new();
    ctl2.dispatch(TurnEvent::Working {
        at: 20_000,
        revision: 1,
    })
    .unwrap();
    ctl2.dispatch(TurnEvent::Cancelled {
        at: 21_000,
        revision: 2,
    })
    .unwrap();
    let cancelled_view = ctl2.view();
    assert_eq!(cancelled_view.state, StateKind::Cancelled);
    assert!(
        ctl2.dispatch(TurnEvent::Cancelled {
            at: 21_000,
            revision: 2
        })
        .is_ok()
    );
    assert_eq!(ctl2.view(), cancelled_view);
    let err = ctl2
        .dispatch(TurnEvent::Completed {
            at: 22_000,
            revision: 3,
        })
        .unwrap_err();
    assert!(matches!(err, TurnError::Sealed { .. }));
    assert_eq!(ctl2.view(), cancelled_view);

    // Completed sealed duplicate.
    let mut ctl3 = ConversationTurnController::new();
    ctl3.dispatch(TurnEvent::Thinking {
        at: 30_000,
        revision: 1,
    })
    .unwrap();
    ctl3.dispatch(TurnEvent::Completed {
        at: 31_000,
        revision: 2,
    })
    .unwrap();
    let completed_view = ctl3.view();
    assert!(
        ctl3.dispatch(TurnEvent::Completed {
            at: 31_000,
            revision: 2
        })
        .is_ok()
    );
    assert_eq!(ctl3.view(), completed_view);
    let err = ctl3
        .dispatch(TurnEvent::Failed {
            at: 31_500,
            revision: 3,
            kind: None,
        })
        .unwrap_err();
    assert!(matches!(err, TurnError::Sealed { .. }));
}

#[test]
fn timestamp_regression_and_stale_revision_are_atomic_typed_refusals() {
    let mut ctl = ConversationTurnController::new();
    ctl.dispatch(TurnEvent::Thinking {
        at: 1_000,
        revision: 5,
    })
    .unwrap();
    let view_before = ctl.view();

    // Timestamp regression.
    let err = ctl
        .dispatch(TurnEvent::Working {
            at: 999,
            revision: 6,
        })
        .unwrap_err();
    assert!(matches!(
        err,
        TurnError::TimestampRegression {
            expected_at_least: 1_000,
            got: 999
        }
    ));
    assert_eq!(ctl.view(), view_before);
    assert_eq!(ctl.revision(), 5);

    // Stale revision.
    let err2 = ctl
        .dispatch(TurnEvent::Working {
            at: 1_100,
            revision: 4,
        })
        .unwrap_err();
    assert!(matches!(
        err2,
        TurnError::StaleRevision {
            expected_at_least: 5,
            got: 4
        }
    ));
    assert_eq!(ctl.view(), view_before);

    // Successful dispatch advances both.
    ctl.dispatch(TurnEvent::Working {
        at: 1_100,
        revision: 6,
    })
    .unwrap();
    assert_eq!(ctl.revision(), 6);
    assert_eq!(ctl.phase_started_at(), Some(1_100));

    // Regression after advance is still atomic.
    let before2 = ctl.view();
    let err3 = ctl
        .dispatch(TurnEvent::StreamingReply {
            at: 1_050,
            revision: 7,
        })
        .unwrap_err();
    assert!(matches!(err3, TurnError::TimestampRegression { .. }));
    assert_eq!(ctl.view(), before2);
}

#[test]
fn signed_extreme_elapsed_arithmetic_cannot_wrap() {
    // MAX - MIN should saturate to u64::MAX without wrapping i64.
    let mut ctl = ConversationTurnController::new();
    ctl.dispatch(TurnEvent::Working {
        at: i64::MIN,
        revision: 1,
    })
    .unwrap();
    ctl.dispatch(TurnEvent::Completed {
        at: i64::MAX,
        revision: 2,
    })
    .unwrap();
    match ctl.view().narration {
        TurnNarration::WorkedFor { elapsed_ms } => assert_eq!(elapsed_ms, u64::MAX),
        other => panic!("expected WorkedFor with u64::MAX, got {other:?}"),
    }

    // Negative elapsed (end before start) saturates to 0, but regression
    // prevents going backwards; test the helper via a fresh controller where
    // start is MAX and end is MIN via direct construction: we cannot dispatch
    // backwards, so we test the view derivation with a controller that starts
    // at MAX and completes at MAX (0), and a controller where start > end is
    // impossible due to monotonic checks. Instead verify that a normal
    // completion with small elapsed does not wrap.
    let mut ctl2 = ConversationTurnController::new();
    ctl2.dispatch(TurnEvent::Thinking {
        at: i64::MAX - 10,
        revision: 1,
    })
    .unwrap();
    ctl2.dispatch(TurnEvent::Completed {
        at: i64::MAX,
        revision: 2,
    })
    .unwrap();
    match ctl2.view().narration {
        TurnNarration::ThoughtFor { elapsed_ms } => assert_eq!(elapsed_ms, 10),
        other => panic!("expected ThoughtFor 10, got {other:?}"),
    }

    // Also test interrupted elapsed at extremes.
    let mut ctl3 = ConversationTurnController::new();
    ctl3.dispatch(TurnEvent::Working {
        at: i64::MIN,
        revision: 1,
    })
    .unwrap();
    ctl3.dispatch(TurnEvent::Interrupted {
        at: i64::MAX,
        revision: 2,
    })
    .unwrap();
    match ctl3.view().narration {
        TurnNarration::Interrupted { elapsed_ms } => assert_eq!(elapsed_ms, u64::MAX),
        other => panic!("expected Interrupted u64::MAX, got {other:?}"),
    }

    // Failed with extremes.
    let mut ctl4 = ConversationTurnController::new();
    ctl4.dispatch(TurnEvent::Working {
        at: i64::MIN,
        revision: 1,
    })
    .unwrap();
    ctl4.dispatch(TurnEvent::Failed {
        at: i64::MAX,
        revision: 2,
        kind: None,
    })
    .unwrap();
    match ctl4.view().narration {
        TurnNarration::Failed { elapsed_ms, .. } => assert_eq!(elapsed_ms, u64::MAX),
        other => panic!("expected Failed u64::MAX, got {other:?}"),
    }
}

#[test]
fn public_state_cannot_represent_two_simultaneous_active_labels() {
    // The view's narration is a closed enum; this test proves the API cannot
    // expose two labels at once and that the controller never yields a view
    // where, say, both Thinking and Streaming are visible.
    let mut ctl = ConversationTurnController::new();
    let cases = [
        TurnEvent::WaitingForProvider { at: 1, revision: 1 },
        TurnEvent::Compacting { at: 2, revision: 2 },
        TurnEvent::Thinking { at: 3, revision: 3 },
        TurnEvent::Working { at: 4, revision: 4 },
        TurnEvent::StreamingReply { at: 5, revision: 5 },
        TurnEvent::WaitingForBackground { at: 6, revision: 6 },
    ];
    for event in cases {
        ctl.dispatch(event).unwrap();
        let view = ctl.view();
        // Exactly one state kind and one narration; match exhaustively.
        match (view.state, &view.narration) {
            (StateKind::WaitingForProvider, TurnNarration::WaitingForProvider)
            | (StateKind::Compacting, TurnNarration::Compacting)
            | (StateKind::Thinking, TurnNarration::Thinking)
            | (StateKind::Working, TurnNarration::Working)
            | (StateKind::StreamingReply, TurnNarration::StreamingReply)
            | (StateKind::WaitingForBackground, TurnNarration::WaitingForBackground) => {}
            other => panic!("mismatched single label view: {other:?}"),
        }
        // Ensure no view ever exposes Statig mutable internals: view is owned
        // value, not a borrow of internal state.
        let view2 = ctl.view();
        assert_eq!(view, view2);
    }

    // Terminal views also have single narration, not two.
    ctl.dispatch(TurnEvent::Completed { at: 7, revision: 7 })
        .unwrap();
    let view = ctl.view();
    assert!(view.narration.is_terminal());
    // Hidden/quiet is only Pending.
    let pending = ConversationTurnController::new();
    assert_eq!(pending.view().narration, TurnNarration::Hidden);
}

#[test]
fn view_includes_exact_state_kind_revision_and_timestamps() {
    let mut ctl = ConversationTurnController::new();
    assert_eq!(ctl.view().state, StateKind::Pending);
    assert_eq!(ctl.view().revision, 0);
    assert_eq!(ctl.view().started_at, None);
    assert_eq!(ctl.view().phase_started_at, None);
    assert_eq!(ctl.view().terminal_at, None);

    ctl.dispatch(TurnEvent::Thinking {
        at: 500,
        revision: 10,
    })
    .unwrap();
    let v = ctl.view();
    assert_eq!(v.state, StateKind::Thinking);
    assert_eq!(v.revision, 10);
    assert_eq!(v.started_at, Some(500));
    assert_eq!(v.phase_started_at, Some(500));
    assert_eq!(v.terminal_at, None);

    ctl.dispatch(TurnEvent::Working {
        at: 600,
        revision: 11,
    })
    .unwrap();
    let v2 = ctl.view();
    assert_eq!(v2.state, StateKind::Working);
    assert_eq!(v2.revision, 11);
    assert_eq!(v2.started_at, Some(500));
    assert_eq!(v2.phase_started_at, Some(600));

    ctl.dispatch(TurnEvent::Completed {
        at: 800,
        revision: 12,
    })
    .unwrap();
    let v3 = ctl.view();
    assert_eq!(v3.state, StateKind::Completed);
    assert_eq!(v3.revision, 12);
    assert_eq!(v3.started_at, Some(500));
    assert_eq!(v3.phase_started_at, Some(800));
    assert_eq!(v3.terminal_at, Some(800));
}

#[test]
fn reentering_active_leaf_updates_phase_but_preserves_origin_and_evidence() {
    let mut ctl = ConversationTurnController::new();
    ctl.dispatch(TurnEvent::Thinking {
        at: 1_000,
        revision: 1,
    })
    .unwrap();
    assert_eq!(ctl.started_at(), Some(1_000));
    assert_eq!(ctl.phase_started_at(), Some(1_000));
    ctl.dispatch(TurnEvent::Thinking {
        at: 1_500,
        revision: 2,
    })
    .unwrap();
    assert_eq!(ctl.started_at(), Some(1_000));
    assert_eq!(ctl.phase_started_at(), Some(1_500));
    assert!(ctl.reasoning_seen());

    ctl.dispatch(TurnEvent::Working {
        at: 2_000,
        revision: 3,
    })
    .unwrap();
    assert_eq!(ctl.started_at(), Some(1_000));
    assert_eq!(ctl.phase_started_at(), Some(2_000));
}
