//! Black-box tests for the registered conversation disclosure and viewport
//! state-machine API.

use artisan_domain::ItemId;
use artisan_frontend::conversation_view_machine::{
    CompletionRejection, Disclosure, DisclosureController, DisclosureEffect, DisclosureEvent,
    DisclosureState, ViewportAnchor, ViewportController, ViewportEffect, ViewportEvent,
    ViewportGeneration, ViewportState,
};

fn item_id(value: &str) -> ItemId {
    ItemId::parse(value).expect("test item id must be valid")
}

fn request_generation(effects: &[ViewportEffect]) -> ViewportGeneration {
    effects
        .iter()
        .find_map(|effect| match effect {
            ViewportEffect::RequestBottomScroll { generation }
            | ViewportEffect::RequestAnchorRestore { generation, .. } => Some(*generation),
            ViewportEffect::None
            | ViewportEffect::ShowJumpToLatest
            | ViewportEffect::HideJumpToLatest
            | ViewportEffect::InvalidateRender
            | ViewportEffect::CompletionRejected { .. }
            | ViewportEffect::GenerationExhausted => None,
        })
        .expect("scroll request must include a generation")
}

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

    let mut active2 = DisclosureController::new(true);
    active2.handle(DisclosureEvent::WorkFailedOrInterrupted);
    assert_eq!(active2.state(), DisclosureState::AutoClosed);
}

#[test]
fn user_open_remains_open_through_terminal_changes() {
    let mut ctrl = DisclosureController::new(true);
    ctrl.handle(DisclosureEvent::UserOpen);
    assert_eq!(ctrl.state(), DisclosureState::UserOpen);
    assert!(ctrl.is_open());
    assert!(ctrl.is_user_controlled());

    ctrl.handle(DisclosureEvent::WorkSettledSuccessfully);
    ctrl.handle(DisclosureEvent::WorkFailedOrInterrupted);
    ctrl.handle(DisclosureEvent::WorkBecameActive);
    assert_eq!(ctrl.state(), DisclosureState::UserOpen);
    assert!(ctrl.is_open());
}

#[test]
fn user_closed_remains_closed_if_work_resumes() {
    let mut ctrl = DisclosureController::new(false);
    ctrl.handle(DisclosureEvent::UserClose);
    assert_eq!(ctrl.state(), DisclosureState::UserClosed);
    assert!(!ctrl.is_open());

    ctrl.handle(DisclosureEvent::WorkBecameActive);
    ctrl.handle(DisclosureEvent::WorkSettledSuccessfully);
    ctrl.handle(DisclosureEvent::WorkFailedOrInterrupted);
    assert_eq!(ctrl.state(), DisclosureState::UserClosed);
    assert!(ctrl.is_user_controlled());
}

#[test]
fn disclosure_removal_transitions_to_explicit_retired_leaf() {
    let mut ctrl = DisclosureController::new(true);
    assert_eq!(
        ctrl.handle(DisclosureEvent::Removed),
        DisclosureEffect::Retired
    );
    assert_eq!(ctrl.state(), DisclosureState::Retired);
    assert!(ctrl.is_retired());
    assert_eq!(ctrl.disclosure(), Disclosure::Closed);

    let state = ctrl.state();
    assert_eq!(
        ctrl.handle(DisclosureEvent::UserToggle),
        DisclosureEffect::None
    );
    assert_eq!(ctrl.state(), state);
    assert!(ctrl.is_retired());
}

#[test]
fn disclosure_constructor_seeding_is_handled_by_the_machine() {
    for state in [
        DisclosureState::AutoOpen,
        DisclosureState::AutoClosed,
        DisclosureState::UserOpen,
        DisclosureState::UserClosed,
        DisclosureState::Retired,
    ] {
        let ctrl = DisclosureController::from_state(state);
        assert_eq!(ctrl.state(), state);
        assert_eq!(ctrl.is_user_controlled(), state.is_user());
    }
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
        assert!(matches!(
            state.disclosure(),
            Disclosure::Open | Disclosure::Closed
        ));
    }

    let retired = DisclosureState::Retired;
    assert!(!retired.is_auto());
    assert!(!retired.is_user());
    assert!(retired.is_retired());
}

// ---------------------------------------------------------------------------
// Viewport tests
// ---------------------------------------------------------------------------

#[test]
fn following_content_emits_one_bottom_follow_request() {
    let mut vp = ViewportController::new();
    assert_eq!(vp.state(), ViewportState::Following);

    let effects = vp.handle(ViewportEvent::ExtentChanged);
    let count = effects
        .iter()
        .filter(|effect| matches!(effect, ViewportEffect::RequestBottomScroll { .. }))
        .count();
    assert_eq!(
        count, 1,
        "following extent must request exactly one bottom scroll"
    );
    assert_eq!(vp.state(), ViewportState::Following);
}

#[test]
fn detached_content_changes_never_emit_scroll_commands() {
    let mut vp = ViewportController::new();
    vp.handle(ViewportEvent::UserScrolled { at_bottom: false });
    assert_eq!(vp.state(), ViewportState::Detached);

    let effects = vp.handle(ViewportEvent::ExtentChanged);
    assert!(!effects.iter().any(|effect| matches!(
        effect,
        ViewportEffect::RequestBottomScroll { .. } | ViewportEffect::RequestAnchorRestore { .. }
    )));
    assert_eq!(vp.state(), ViewportState::Detached);
}

#[test]
fn exact_item_anchor_offset_restoration_survives_extent_changes() {
    let anchor = item_id("msg-123");
    let mut vp = ViewportController::anchored(anchor.clone(), -42);
    assert!(matches!(
        vp.state(),
        ViewportState::Anchored { anchor_id, offset }
            if anchor_id == anchor && offset == -42
    ));

    let effects = vp.handle(ViewportEvent::ExtentChanged);
    let (restored_anchor, restored_offset, generation) = effects
        .iter()
        .find_map(|effect| match effect {
            ViewportEffect::RequestAnchorRestore {
                anchor_id,
                offset,
                generation,
            } => Some((anchor_id.clone(), *offset, *generation)),
            ViewportEffect::None
            | ViewportEffect::RequestBottomScroll { .. }
            | ViewportEffect::ShowJumpToLatest
            | ViewportEffect::HideJumpToLatest
            | ViewportEffect::InvalidateRender
            | ViewportEffect::CompletionRejected { .. }
            | ViewportEffect::GenerationExhausted => None,
        })
        .expect("anchored extent must emit anchor restore");
    assert_eq!(restored_anchor, anchor);
    assert_eq!(restored_offset, -42);
    assert!(generation.value() > 0);
    assert_eq!(
        vp.state(),
        ViewportState::Anchored {
            anchor_id: anchor,
            offset: -42,
        }
    );

    let second_generation = request_generation(&vp.handle(ViewportEvent::ExtentChanged));
    assert!(second_generation.value() > generation.value());
}

#[test]
fn viewport_anchor_and_events_use_domain_item_ids() {
    let anchor = ViewportAnchor::new(item_id("anchor-A"), 10);
    assert_eq!(anchor.anchor_id, item_id("anchor-A"));
    assert_eq!(anchor.offset, 10);

    let event = ViewportEvent::anchor_observed(item_id("anchor-A"), 10);
    assert_eq!(
        event,
        ViewportEvent::AnchorObserved {
            anchor_id: item_id("anchor-A"),
            offset: 10,
        }
    );
}

#[test]
fn exact_item_anchor_removal_detaches_without_guessing_a_neighbor() {
    let anchor = item_id("anchor-A");
    let mut vp = ViewportController::anchored(anchor.clone(), 10);
    let effects = vp.handle(ViewportEvent::anchor_removed(anchor.clone()));

    assert_eq!(vp.state(), ViewportState::Detached);
    assert!(effects
        .iter()
        .any(|effect| matches!(effect, ViewportEffect::ShowJumpToLatest)));

    let mut vp2 = ViewportController::anchored(anchor.clone(), 10);
    let effects2 = vp2.handle(ViewportEvent::anchor_removed(item_id("other")));
    assert_eq!(
        vp2.state(),
        ViewportState::Anchored {
            anchor_id: anchor,
            offset: 10,
        }
    );
    assert!(!effects2
        .iter()
        .any(|effect| matches!(effect, ViewportEffect::ShowJumpToLatest)));
}

#[test]
fn jump_to_bottom_uses_generation_fenced_scrolling_settling_following() {
    let mut vp = ViewportController::new();
    let effects = vp.handle(ViewportEvent::JumpToBottomRequested);
    let generation = request_generation(&effects);
    assert_eq!(vp.state(), ViewportState::Scrolling { generation });
    assert!(effects
        .iter()
        .any(|effect| matches!(effect, ViewportEffect::HideJumpToLatest)));

    let effects2 = vp.handle(ViewportEvent::scroll_completed(generation));
    assert!(!effects2
        .iter()
        .any(|effect| matches!(effect, ViewportEffect::CompletionRejected { .. })));
    assert_eq!(vp.state(), ViewportState::Settling { generation });

    let effects3 = vp.handle(ViewportEvent::LayoutSettled);
    assert_eq!(vp.state(), ViewportState::Following);
    assert!(effects3
        .iter()
        .any(|effect| matches!(effect, ViewportEffect::HideJumpToLatest)));
}

#[test]
fn layout_settled_while_scrolling_does_not_finish_the_active_generation() {
    let mut vp = ViewportController::new();
    let generation = request_generation(&vp.handle(ViewportEvent::JumpToBottomRequested));

    let effects = vp.handle(ViewportEvent::LayoutSettled);
    assert_eq!(effects, vec![ViewportEffect::None]);
    assert_eq!(vp.state(), ViewportState::Scrolling { generation });

    vp.handle(ViewportEvent::ScrollCompleted { generation });
    assert_eq!(vp.state(), ViewportState::Settling { generation });
}

#[test]
fn stale_completions_are_atomic_typed_refusals() {
    let mut vp = ViewportController::new();
    let gen1 = request_generation(&vp.handle(ViewportEvent::JumpToBottomRequested));
    let gen2 = request_generation(&vp.handle(ViewportEvent::JumpToBottomRequested));
    assert!(gen2.value() > gen1.value());
    assert_eq!(vp.state(), ViewportState::Scrolling { generation: gen2 });

    let effects_stale = vp.handle(ViewportEvent::ScrollCompleted { generation: gen1 });
    assert!(effects_stale.iter().any(|effect| matches!(
        effect,
        ViewportEffect::CompletionRejected {
            generation,
            reason: CompletionRejection::StaleGeneration,
        } if *generation == gen1
    )));
    assert_eq!(vp.state(), ViewportState::Scrolling { generation: gen2 });

    let duplicate = vp.handle(ViewportEvent::ScrollCompleted { generation: gen1 });
    assert!(duplicate
        .iter()
        .any(|effect| matches!(effect, ViewportEffect::CompletionRejected { .. })));

    let current = vp.handle(ViewportEvent::ScrollCompleted { generation: gen2 });
    assert!(!current
        .iter()
        .any(|effect| matches!(effect, ViewportEffect::CompletionRejected { .. })));
    assert_eq!(vp.state(), ViewportState::Settling { generation: gen2 });
}

#[test]
fn programmatic_start_is_fenced_and_can_complete_the_generated_request() {
    let mut vp = ViewportController::new();
    let stale = ViewportGeneration::new(99);
    let rejected = vp.handle(ViewportEvent::programmatic_scroll_started(stale));
    assert!(rejected.iter().any(|effect| matches!(
        effect,
        ViewportEffect::CompletionRejected {
            generation,
            reason: CompletionRejection::StaleGeneration,
        } if *generation == stale
    )));
    assert_eq!(vp.state(), ViewportState::Following);

    let request = vp.handle(ViewportEvent::ExtentChanged);
    let generation = request_generation(&request);
    let started = vp.handle(ViewportEvent::programmatic_scroll_started(generation));
    assert_eq!(vp.state(), ViewportState::Scrolling { generation });
    assert!(started
        .iter()
        .any(|effect| matches!(effect, ViewportEffect::InvalidateRender)));
}

#[test]
fn generation_overflow_never_wraps() {
    let max = ViewportGeneration::new(u64::MAX);
    let mut vp = ViewportController::seeded_for_test(max);
    let effects = vp.handle(ViewportEvent::JumpToBottomRequested);
    assert!(effects
        .iter()
        .any(|effect| matches!(effect, ViewportEffect::GenerationExhausted)));
    assert_eq!(vp.generation(), max);
    assert_eq!(vp.state(), ViewportState::Following);

    let effects2 = vp.handle(ViewportEvent::ExtentChanged);
    assert!(effects2
        .iter()
        .any(|effect| matches!(effect, ViewportEffect::GenerationExhausted)));

    let mut anchored = ViewportController::seeded_for_test(max);
    anchored.handle(ViewportEvent::anchor_observed(item_id("a"), 0));
    let effects3 = anchored.handle(ViewportEvent::ExtentChanged);
    assert!(effects3
        .iter()
        .any(|effect| matches!(effect, ViewportEffect::GenerationExhausted)));
    assert_eq!(anchored.generation(), max);
    assert!(matches!(anchored.state(), ViewportState::Anchored { .. }));
}

#[test]
fn no_viewport_state_can_be_two_leaves_at_once() {
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
            anchor_id: item_id("x"),
            offset: 0,
        },
        ViewportState::Closed,
    ] {
        let true_count = [
            state.is_following(),
            state.is_detached(),
            state.is_anchored(),
            state.is_scrolling(),
            state.is_settling(),
            state.is_closed(),
        ]
        .into_iter()
        .filter(|value| *value)
        .count();
        assert_eq!(true_count, 1, "{state:?} must be exactly one viewport leaf");
    }
}

#[test]
fn detached_never_jumps_on_token_arrival_and_following_always_jumps() {
    let mut detached = ViewportController::new();
    detached.handle(ViewportEvent::UserScrolled { at_bottom: false });
    for _ in 0..3 {
        let effects = detached.handle(ViewportEvent::ExtentChanged);
        assert!(!effects
            .iter()
            .any(|effect| matches!(effect, ViewportEffect::RequestBottomScroll { .. })));
    }

    let mut following = ViewportController::new();
    for _ in 0..3 {
        let effects = following.handle(ViewportEvent::ExtentChanged);
        assert!(effects
            .iter()
            .any(|effect| matches!(effect, ViewportEffect::RequestBottomScroll { .. })));
    }
}

#[test]
fn closed_is_terminal_for_viewport() {
    let mut vp = ViewportController::new();
    vp.handle(ViewportEvent::OwnerClosed);
    assert_eq!(vp.state(), ViewportState::Closed);

    let effects = vp.handle(ViewportEvent::ExtentChanged);
    assert!(effects
        .iter()
        .all(|effect| matches!(effect, ViewportEffect::None)));
    let effects2 = vp.handle(ViewportEvent::JumpToBottomRequested);
    assert!(effects2
        .iter()
        .all(|effect| matches!(effect, ViewportEffect::None)));
    assert_eq!(vp.state(), ViewportState::Closed);
}
