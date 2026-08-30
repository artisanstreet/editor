//! Black-box tests for conversation disclosure and viewport machines.
//!
//! The production module is included directly so this focused harness
//! stays dependency-free and does not require shared crate or build-file
//! registration.

#[path = "../../modules/frontend/src/conversation_view_machine.rs"]
mod conversation_view_machine;

use conversation_view_machine::{
    CompletionRejection, Disclosure, DisclosureController, DisclosureEffect, DisclosureEvent,
    DisclosureState, ViewportController, ViewportEffect, ViewportEvent, ViewportGeneration,
    ViewportState,
};

// ---------------------------------------------------------------------------
// Disclosure tests
// ---------------------------------------------------------------------------

#[test]
fn active_disclosure_auto_opens_and_terminal_auto_closes() {
    let mut active = DisclosureController::new(true);
    assert_eq!(active.state(), DisclosureState::AutoOpen);
    assert_eq!(active.disclosure(), Disclosure::Open);
    assert!(active.is_open());

    let effect = active.handle(DisclosureEvent::WorkSettledSuccessfully);
    assert_eq!(effect, DisclosureEffect::None);
    assert_eq!(active.state(), DisclosureState::AutoClosed);
    assert_eq!(active.disclosure(), Disclosure::Closed);
    assert!(!active.is_open());

    let mut settled = DisclosureController::new(false);
    assert_eq!(settled.state(), DisclosureState::AutoClosed);
    assert!(!settled.is_open());

    let effect = settled.handle(DisclosureEvent::WorkBecameActive);
    assert_eq!(effect, DisclosureEffect::None);
    assert_eq!(settled.state(), DisclosureState::AutoOpen);
    assert!(settled.is_open());

    // Failed/interrupted also closes only when auto.
    let mut active2 = DisclosureController::new(true);
    active2.handle(DisclosureEvent::WorkFailedOrInterrupted);
    assert_eq!(active2.state(), DisclosureState::AutoClosed);

    // View is a single enum, never two booleans.
    assert!(matches!(
        active.disclosure(),
        Disclosure::Open | Disclosure::Closed
    ));
}

#[test]
fn user_open_remains_open_through_terminal_changes() {
    let mut ctrl = DisclosureController::new(true);
    ctrl.handle(DisclosureEvent::UserOpen);
    assert_eq!(ctrl.state(), DisclosureState::UserOpen);
    assert!(ctrl.is_open());

    ctrl.handle(DisclosureEvent::WorkSettledSuccessfully);
    assert_eq!(ctrl.state(), DisclosureState::UserOpen);
    assert!(ctrl.is_open());

    ctrl.handle(DisclosureEvent::WorkFailedOrInterrupted);
    assert_eq!(ctrl.state(), DisclosureState::UserOpen);
    assert!(ctrl.is_open());

    // Even if work becomes active again, user choice persists.
    ctrl.handle(DisclosureEvent::WorkBecameActive);
    assert_eq!(ctrl.state(), DisclosureState::UserOpen);
}

#[test]
fn user_closed_remains_closed_if_work_resumes() {
    let mut ctrl = DisclosureController::new(false);
    assert_eq!(ctrl.state(), DisclosureState::AutoClosed);

    ctrl.handle(DisclosureEvent::UserClose);
    assert_eq!(ctrl.state(), DisclosureState::UserClosed);
    assert!(!ctrl.is_open());

    ctrl.handle(DisclosureEvent::WorkBecameActive);
    assert_eq!(
        ctrl.state(),
        DisclosureState::UserClosed,
        "user choice wins over later automatic lifecycle changes"
    );
    assert!(!ctrl.is_open());

    // Terminal events also must not flip a user-closed card.
    ctrl.handle(DisclosureEvent::WorkSettledSuccessfully);
    assert_eq!(ctrl.state(), DisclosureState::UserClosed);
    ctrl.handle(DisclosureEvent::WorkFailedOrInterrupted);
    assert_eq!(ctrl.state(), DisclosureState::UserClosed);
}

#[test]
fn removal_is_terminal_and_emits_retirement_effect() {
    let mut ctrl = DisclosureController::new(true);
    let effect = ctrl.handle(DisclosureEvent::Removed);
    assert_eq!(effect, DisclosureEffect::Retired);
    assert!(ctrl.is_retired());

    // After retirement further events are ignored.
    let effect2 = ctrl.handle(DisclosureEvent::UserToggle);
    assert_eq!(effect2, DisclosureEffect::None);
    assert!(ctrl.is_retired());
}

#[test]
fn disclosure_view_never_simultaneously_auto_and_user() {
    for state in [
        DisclosureState::AutoOpen,
        DisclosureState::AutoClosed,
        DisclosureState::UserOpen,
        DisclosureState::UserClosed,
    ] {
        assert!(
            state.is_auto() ^ state.is_user(),
            "{state:?} must be exactly auto or user"
        );
        let disc = state.disclosure();
        assert!(matches!(disc, Disclosure::Open | Disclosure::Closed));
    }

    let mut ctrl = DisclosureController::from_state(DisclosureState::AutoOpen);
    assert!(!ctrl.is_user_controlled());
    ctrl.handle(DisclosureEvent::UserToggle);
    assert!(ctrl.is_user_controlled());
    assert_eq!(ctrl.state(), DisclosureState::UserClosed);
}

// ---------------------------------------------------------------------------
// Viewport tests
// ---------------------------------------------------------------------------

#[test]
fn following_content_emits_one_bottom_follow_request() {
    let mut vp = ViewportController::new();
    assert!(matches!(vp.state(), ViewportState::Following));

    let effects = vp.handle(ViewportEvent::ExtentChanged);
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, ViewportEffect::RequestBottomScroll { .. })),
        "following extent must emit one bottom-follow request, got {effects:?}"
    );
    let count = effects
        .iter()
        .filter(|e| matches!(e, ViewportEffect::RequestBottomScroll { .. }))
        .count();
    assert_eq!(count, 1, "exactly one bottom-follow request");
    assert!(matches!(vp.state(), ViewportState::Following));
}

#[test]
fn detached_content_changes_never_emit_scroll_commands() {
    let mut vp = ViewportController::new();
    vp.handle(ViewportEvent::UserScrolled { at_bottom: false });
    assert!(matches!(vp.state(), ViewportState::Detached));

    let effects = vp.handle(ViewportEvent::ExtentChanged);
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, ViewportEffect::RequestBottomScroll { .. })),
        "detached must never emit bottom scroll, got {effects:?}"
    );
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, ViewportEffect::RequestAnchorRestore { .. })),
        "detached must never emit anchor restore, got {effects:?}"
    );
    assert!(matches!(vp.state(), ViewportState::Detached));
}

#[test]
fn exact_anchor_offset_restoration_survives_extent_changes() {
    let mut vp = ViewportController::anchored("msg-123", -42);
    assert!(matches!(
        vp.state(),
        ViewportState::Anchored { anchor_id, offset } if anchor_id == "msg-123" && *offset == -42
    ));

    let effects = vp.handle(ViewportEvent::ExtentChanged);
    let restore = effects.iter().find_map(|e| match e {
        ViewportEffect::RequestAnchorRestore {
            anchor_id,
            offset,
            generation,
        } => Some((anchor_id.clone(), *offset, *generation)),
        _ => None,
    });
    let (anchor_id, offset, generation_value) =
        restore.expect("anchored extent must emit anchor restore");
    assert_eq!(anchor_id, "msg-123");
    assert_eq!(offset, -42);
    assert!(generation_value.value() > 0);
    // Still anchored to the exact same item.
    assert!(matches!(
        vp.state(),
        ViewportState::Anchored { anchor_id, offset } if anchor_id == "msg-123" && *offset == -42
    ));

    // A second extent also preserves the exact anchor, with a monotonic
    // generation.
    let effects2 = vp.handle(ViewportEvent::ExtentChanged);
    let (_, _, gen2) = effects2
        .iter()
        .find_map(|e| match e {
            ViewportEffect::RequestAnchorRestore { generation, .. } => Some(generation),
            _ => None,
        })
        .map(|g| (String::new(), 0, *g))
        .unwrap_or_else(|| panic!("second restore missing: {effects2:?}"));
    // Check monotonic increase via collected generations.
    assert!(gen2.value() > generation_value.value());
}

#[test]
fn anchor_removal_becomes_detached_without_guessing_another_anchor() {
    let mut vp = ViewportController::anchored("anchor-A", 10);
    let effects = vp.handle(ViewportEvent::AnchorRemoved {
        anchor_id: "anchor-A".to_owned(),
    });

    assert!(matches!(vp.state(), ViewportState::Detached));
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, ViewportEffect::ShowJumpToLatest)),
        "removal must show jump-to-latest, got {effects:?}"
    );
    // Must not be anchored to any neighbor heuristically.
    assert!(!matches!(vp.state(), ViewportState::Anchored { .. }));
    // Removing a non-matching anchor is a no-op.
    let mut vp2 = ViewportController::anchored("anchor-A", 10);
    let effects2 = vp2.handle(ViewportEvent::AnchorRemoved {
        anchor_id: "other".to_owned(),
    });
    assert!(matches!(
        vp2.state(),
        ViewportState::Anchored { anchor_id, .. } if anchor_id == "anchor-A"
    ));
    assert!(
        !effects2
            .iter()
            .any(|e| matches!(e, ViewportEffect::ShowJumpToLatest))
    );
}

#[test]
fn jump_to_bottom_uses_generation_fenced_scrolling_settling_following() {
    let mut vp = ViewportController::new();
    let effects = vp.handle(ViewportEvent::JumpToBottomRequested);
    let generation_value = match effects.iter().find_map(|e| match e {
        ViewportEffect::RequestBottomScroll { generation } => Some(*generation),
        _ => None,
    }) {
        Some(g) => g,
        None => panic!("jump must emit RequestBottomScroll, got {effects:?}"),
    };
    assert!(matches!(
        vp.state(),
        ViewportState::Scrolling { generation } if generation == generation_value
    ));
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, ViewportEffect::HideJumpToLatest))
    );

    // Matching completion moves to settling, not directly to following.
    let effects2 = vp.handle(ViewportEvent::ScrollCompleted {
        generation: generation_value,
    });
    assert!(
        !effects2
            .iter()
            .any(|e| matches!(e, ViewportEffect::CompletionRejected { .. })),
        "matching completion must not be rejected"
    );
    assert!(matches!(
        vp.state(),
        ViewportState::Settling { generation } if generation == generation_value
    ));

    // Only after layout settled do we return to following.
    let effects3 = vp.handle(ViewportEvent::LayoutSettled);
    assert!(matches!(vp.state(), ViewportState::Following));
    assert!(
        effects3
            .iter()
            .any(|e| matches!(e, ViewportEffect::HideJumpToLatest))
    );
}

#[test]
fn stale_completions_are_atomic_typed_refusals() {
    let mut vp = ViewportController::new();
    let effects1 = vp.handle(ViewportEvent::JumpToBottomRequested);
    let gen1 = effects1
        .iter()
        .find_map(|e| match e {
            ViewportEffect::RequestBottomScroll { generation } => Some(*generation),
            _ => None,
        })
        .unwrap();

    // Start a newer fenced command before the old one completes.
    let effects2 = vp.handle(ViewportEvent::JumpToBottomRequested);
    let gen2 = effects2
        .iter()
        .find_map(|e| match e {
            ViewportEffect::RequestBottomScroll { generation } => Some(*generation),
            _ => None,
        })
        .unwrap();
    assert!(gen2.value() > gen1.value());
    assert!(matches!(
        vp.state(),
        ViewportState::Scrolling { generation } if generation == gen2
    ));

    // Stale completion must be a typed refusal and must not clear newer scroll.
    let effects_stale = vp.handle(ViewportEvent::ScrollCompleted { generation: gen1 });
    assert!(
        effects_stale.iter().any(|e| matches!(
            e,
            ViewportEffect::CompletionRejected {
                generation,
                reason: CompletionRejection::StaleGeneration
            } if *generation == gen1
        )),
        "stale completion must be typed refusal, got {effects_stale:?}"
    );
    assert!(matches!(
        vp.state(),
        ViewportState::Scrolling { generation } if generation == gen2
    ));

    // Duplicate completion of the stale generation is also rejected.
    let dup = vp.handle(ViewportEvent::ScrollCompleted { generation: gen1 });
    assert!(
        dup.iter()
            .any(|e| matches!(e, ViewportEffect::CompletionRejected { .. }))
    );

    // The current generation still completes normally.
    let ok = vp.handle(ViewportEvent::ScrollCompleted { generation: gen2 });
    assert!(
        !ok.iter()
            .any(|e| matches!(e, ViewportEffect::CompletionRejected { .. }))
    );
    assert!(matches!(
        vp.state(),
        ViewportState::Settling { generation } if generation == gen2
    ));
}

#[test]
fn generation_overflow_never_wraps() {
    let mut vp = ViewportController::new().with_generation(ViewportGeneration::new(u64::MAX));
    let effects = vp.handle(ViewportEvent::JumpToBottomRequested);
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, ViewportEffect::GenerationExhausted)),
        "overflow must be typed GenerationExhausted, got {effects:?}"
    );
    // Must not have wrapped and must not have entered scrolling.
    assert_eq!(vp.generation(), ViewportGeneration::new(u64::MAX));
    assert!(matches!(vp.state(), ViewportState::Following));

    // Extent in following also must not wrap.
    let effects2 = vp.handle(ViewportEvent::ExtentChanged);
    assert!(
        effects2
            .iter()
            .any(|e| matches!(e, ViewportEffect::GenerationExhausted))
    );

    // Anchored path also respects overflow.
    let mut vp2 =
        ViewportController::anchored("a", 0).with_generation(ViewportGeneration::new(u64::MAX));
    let effects3 = vp2.handle(ViewportEvent::ExtentChanged);
    assert!(
        effects3
            .iter()
            .any(|e| matches!(e, ViewportEffect::GenerationExhausted))
    );
}

#[test]
fn no_state_view_can_simultaneously_be_following_and_detached() {
    for state in [
        ViewportState::Following,
        ViewportState::Detached,
        ViewportState::Scrolling {
            generation: ViewportGeneration::new(1),
        },
        ViewportState::Settling {
            generation: ViewportGeneration::new(1),
        },
        ViewportState::Anchored {
            anchor_id: "x".to_owned(),
            offset: 0,
        },
        ViewportState::Closed,
    ] {
        let is_following = state.is_following();
        let is_detached = state.is_detached();
        let is_anchored = state.is_anchored();
        let is_scrolling = state.is_scrolling();
        let is_settling = state.is_settling();
        let is_closed = state.is_closed();

        let true_count = [
            is_following,
            is_detached,
            is_anchored,
            is_scrolling,
            is_settling,
            is_closed,
        ]
        .iter()
        .filter(|v| **v)
        .count();
        assert_eq!(
            true_count, 1,
            "{state:?} must be exactly one exclusive view, got {true_count}"
        );
        assert!(
            !(is_following && is_detached),
            "no state can be both following and detached: {state:?}"
        );
    }

    // Disclosure exclusivity already checked above; also prove no open+closed.
    let open = Disclosure::Open;
    let closed = Disclosure::Closed;
    assert!(open.is_open() && !open.is_closed());
    assert!(closed.is_closed() && !closed.is_open());
}

#[test]
fn detached_never_jumps_on_token_arrival_and_following_always_jumps() {
    // This is a second phrasing of the same frozen rule with explicit
    // token-like extent events.
    let mut detached = ViewportController::new();
    detached.handle(ViewportEvent::UserScrolled { at_bottom: false });
    for _ in 0..3 {
        let e = detached.handle(ViewportEvent::ExtentChanged);
        assert!(
            !e.iter()
                .any(|x| matches!(x, ViewportEffect::RequestBottomScroll { .. })),
            "detached token must not cause jump"
        );
    }

    let mut following = ViewportController::new();
    for _ in 0..3 {
        let e = following.handle(ViewportEvent::ExtentChanged);
        assert!(
            e.iter()
                .any(|x| matches!(x, ViewportEffect::RequestBottomScroll { .. })),
            "following token must cause bottom follow"
        );
    }
}

#[test]
fn closed_is_terminal_for_viewport() {
    let mut vp = ViewportController::new();
    vp.handle(ViewportEvent::OwnerClosed);
    assert!(matches!(vp.state(), ViewportState::Closed));

    let effects = vp.handle(ViewportEvent::ExtentChanged);
    assert!(effects.iter().all(|e| matches!(e, ViewportEffect::None)));
    assert!(matches!(vp.state(), ViewportState::Closed));

    let effects2 = vp.handle(ViewportEvent::JumpToBottomRequested);
    assert!(effects2.iter().all(|e| matches!(e, ViewportEffect::None)));
    assert!(matches!(vp.state(), ViewportState::Closed));
}
