//! Dependency-free coverage for the model-policy mutation/reconciliation state.
//!
//! The source module is included directly so this packet can be checked with
//! plain Rust 1.98 without changing shared frontend registration.

#[path = "../../modules/frontend/src/model_policy_controller.rs"]
mod model_policy_controller;

use std::sync::Arc;
use std::sync::mpsc;
use std::thread;

use model_policy_controller::{
    ModelPolicyController, ModelPolicyMutationError, PolicyControllerState, PolicyFlushResult,
    SessionPolicy, SessionPolicyPatch,
};

fn policy(
    engine_id: &str,
    model_id: &str,
    reasoning_effort: &str,
    permission_mode: &str,
    web_search_enabled: bool,
) -> SessionPolicy {
    SessionPolicy::new(
        engine_id,
        model_id,
        reasoning_effort,
        permission_mode,
        web_search_enabled,
    )
}

fn base_policy() -> SessionPolicy {
    policy("engine-a", "model-a", "medium", "on_request", false)
}

fn alternate_policy() -> SessionPolicy {
    policy("engine-b", "model-b", "max", "never", true)
}

fn assert_flush_result(
    result: &PolicyFlushResult,
    confirmed: &[SessionPolicy],
    current: Option<&SessionPolicy>,
) {
    assert_eq!(result.confirmed, confirmed);
    assert_eq!(result.current.as_ref(), current);
    assert_eq!(result.confirmed(), confirmed);
    assert_eq!(result.current(), current);
}

#[test]
fn empty_state_is_absent_and_empty_flush_does_not_persist() {
    let controller = ModelPolicyController::new();

    assert_eq!(controller.current(), None);
    assert_eq!(controller.state(), PolicyControllerState::default());
    assert_eq!(controller.patch(SessionPolicyPatch::empty()), None);

    let calls = std::cell::Cell::new(0);
    let result = controller
        .flush(|policy| {
            calls.set(calls.get() + 1);
            Ok::<_, &'static str>(policy)
        })
        .expect("an empty flush succeeds");

    assert_flush_result(&result, &[], None);
    assert_eq!(calls.get(), 0);
}

#[test]
fn effective_precedence_is_desired_then_in_flight_then_authoritative() {
    let controller = ModelPolicyController::new();
    let authoritative = base_policy();
    let desired = alternate_policy();
    let queued = policy("engine-c", "model-c", "low", "on_request", false);

    assert_eq!(
        controller.set_authoritative(authoritative.clone()),
        authoritative
    );
    assert_eq!(controller.current(), Some(authoritative.clone()));
    assert_eq!(controller.replace(desired.clone()), desired.clone());
    assert_eq!(controller.current(), Some(desired.clone()));

    let mut calls = 0;
    let result = controller
        .flush(|in_flight| {
            calls += 1;
            if calls == 1 {
                assert_eq!(controller.state().desired, None);
                assert_eq!(controller.state().in_flight, Some(desired.clone()));
                assert_eq!(
                    controller.state().authoritative,
                    Some(authoritative.clone())
                );
                assert_eq!(controller.current(), Some(desired.clone()));
                controller.replace(queued.clone());
                assert_eq!(controller.current(), Some(queued.clone()));
                assert_eq!(controller.state().in_flight, Some(desired.clone()));
            }
            Ok::<_, &'static str>(in_flight)
        })
        .expect("both queued policies persist");

    assert_eq!(calls, 2);
    assert_flush_result(&result, &[desired, queued.clone()], Some(&queued));
    assert_eq!(controller.current(), Some(queued));
}

#[test]
fn whole_replacement_discards_prior_axes_while_patches_preserve_omissions() {
    let controller = ModelPolicyController::new();
    let original = base_policy();
    let replacement = alternate_policy();

    controller.set_authoritative(original.clone());
    let patched = controller
        .patch(SessionPolicyPatch::for_web_search_enabled(true))
        .expect("authoritative policy supplies a patch base");
    assert_eq!(
        patched,
        policy("engine-a", "model-a", "medium", "on_request", true)
    );

    assert_eq!(controller.replace(replacement.clone()), replacement.clone());
    assert_eq!(controller.current(), Some(replacement.clone()));

    let patched_replacement = controller
        .patch(SessionPolicyPatch::for_model_id("model-c"))
        .expect("replacement supplies a patch base");
    assert_eq!(
        patched_replacement,
        policy("engine-b", "model-c", "max", "never", true)
    );

    let all_axes = SessionPolicyPatch::default()
        .with_engine_id("engine-c")
        .with_model_id("model-d")
        .with_reasoning_effort("xhigh")
        .with_permission_mode("never")
        .with_web_search_enabled(false);
    assert_eq!(
        controller.patch(all_axes),
        Some(policy("engine-c", "model-d", "xhigh", "never", false))
    );
}

#[test]
fn patch_convenience_constructors_keep_each_axis_independent() {
    let patches = [
        (
            SessionPolicyPatch::for_engine_id("engine-z"),
            policy("engine-z", "model-a", "medium", "on_request", false),
        ),
        (
            SessionPolicyPatch::for_reasoning_effort("ultra"),
            policy("engine-a", "model-a", "ultra", "on_request", false),
        ),
        (
            SessionPolicyPatch::for_permission_mode("never"),
            policy("engine-a", "model-a", "medium", "never", false),
        ),
    ];

    assert!(SessionPolicyPatch::empty().is_empty());
    for (patch, expected) in patches {
        assert!(!patch.is_empty());
        assert_eq!(base_policy().apply_patch(patch), expected);
    }
}

#[test]
fn every_patch_mask_changes_only_the_axes_it_supplies() {
    let original = base_policy();
    let replacements = [
        String::from("engine-z"),
        String::from("model-z"),
        String::from("ultra"),
        String::from("never"),
    ];

    for mask in 0_u8..32 {
        let patch = SessionPolicyPatch {
            engine_id: (mask & 1 != 0).then(|| replacements[0].clone()),
            model_id: (mask & 2 != 0).then(|| replacements[1].clone()),
            reasoning_effort: (mask & 4 != 0).then(|| replacements[2].clone()),
            permission_mode: (mask & 8 != 0).then(|| replacements[3].clone()),
            web_search_enabled: (mask & 16 != 0).then_some(true),
        };
        let expected = SessionPolicy {
            engine_id: if mask & 1 != 0 {
                String::from("engine-z")
            } else {
                original.engine_id.clone()
            },
            model_id: if mask & 2 != 0 {
                String::from("model-z")
            } else {
                original.model_id.clone()
            },
            reasoning_effort: if mask & 4 != 0 {
                String::from("ultra")
            } else {
                original.reasoning_effort.clone()
            },
            permission_mode: if mask & 8 != 0 {
                String::from("never")
            } else {
                original.permission_mode.clone()
            },
            web_search_enabled: mask & 16 != 0,
        };

        assert_eq!(original.clone().apply_patch(patch), expected, "mask={mask}");
    }
}

#[test]
fn authoritative_replacement_resets_only_after_a_real_change() {
    let controller = ModelPolicyController::new();
    let first = base_policy();
    let second = alternate_policy();
    let repair = policy("engine-r", "model-r", "high", "on_request", true);

    controller.set_authoritative(first.clone());
    assert!(controller.request_repair(repair.clone()));
    let first_key = controller
        .state()
        .repair_key
        .expect("repair request stores a key");

    assert_eq!(controller.set_authoritative(first.clone()), repair);
    assert_eq!(
        controller.state().repair_key.as_deref(),
        Some(first_key.as_str())
    );

    assert_eq!(controller.set_authoritative(second), repair);
    assert_eq!(controller.state().repair_key, None);
}

#[test]
fn repair_requests_deduplicate_against_effective_policy_and_existing_key() {
    let controller = ModelPolicyController::new();
    let authoritative = base_policy();
    let repair = alternate_policy();
    let normalized = policy("engine-n", "model-n", "medium", "on_request", false);

    controller.set_authoritative(authoritative.clone());
    assert!(!controller.request_repair(authoritative));
    assert_eq!(controller.state().desired, None);

    assert!(controller.request_repair(repair.clone()));
    assert!(!controller.request_repair(repair.clone()));

    let result = controller
        .flush(|_| Ok::<_, &'static str>(normalized.clone()))
        .expect("repair persists");
    assert_flush_result(
        &result,
        std::slice::from_ref(&normalized),
        Some(&normalized),
    );
    assert_eq!(controller.state().repair_key, Some(repair.key()));

    // The normalized effective value differs from `repair`, so this false
    // result proves that the existing repair key is an independent guard.
    assert!(!controller.request_repair(repair.clone()));

    let changed = policy("engine-c", "model-c", "high", "never", true);
    controller.set_authoritative(changed.clone());
    assert_eq!(controller.current(), Some(changed.clone()));
    assert!(controller.state().repair_key.is_none());
    assert!(!controller.request_repair(changed));
    assert!(controller.request_repair(repair));
}

#[test]
fn one_flush_confirms_normalized_authority_and_a_later_flush_is_empty() {
    let controller = ModelPolicyController::new();
    let desired = base_policy();
    let authoritative = alternate_policy();
    let calls = std::cell::RefCell::new(Vec::new());

    controller.replace(desired.clone());
    let first = controller
        .flush(|policy| {
            calls.borrow_mut().push(policy.clone());
            Ok::<_, &'static str>(authoritative.clone())
        })
        .expect("the first flush succeeds");
    assert_flush_result(
        &first,
        std::slice::from_ref(&authoritative),
        Some(&authoritative),
    );
    assert_eq!(*calls.borrow(), vec![desired]);
    assert_eq!(controller.state().in_flight, None);

    let second = controller
        .flush(|policy| {
            calls.borrow_mut().push(policy.clone());
            Ok::<_, &'static str>(policy)
        })
        .expect("an empty later flush succeeds");
    assert_flush_result(&second, &[], Some(&authoritative));
    assert_eq!(*calls.borrow(), vec![base_policy()]);
}

#[test]
fn one_flush_consumes_latest_intent_queued_during_persistence() {
    let controller = ModelPolicyController::new();
    let first = base_policy();
    let intermediate = policy("engine-i", "model-i", "low", "on_request", true);
    let latest = alternate_policy();
    let normalized_first = policy("engine-n", "model-n", "high", "never", false);
    let mut persisted = Vec::new();

    controller.replace(first.clone());
    let result = controller
        .flush(|policy| {
            persisted.push(policy.clone());
            if policy == first {
                controller.replace(intermediate.clone());
                controller.replace(latest.clone());
                assert_eq!(controller.current(), Some(latest.clone()));
            }
            if policy == first {
                Ok::<_, &'static str>(normalized_first.clone())
            } else {
                Ok::<_, &'static str>(policy)
            }
        })
        .expect("the queued latest intent also succeeds");

    assert_eq!(persisted, vec![first, latest.clone()]);
    assert_flush_result(&result, &[normalized_first, latest.clone()], Some(&latest));
    assert_eq!(controller.state().desired, None);
    assert_eq!(controller.state().in_flight, None);
}

#[test]
fn persistence_failure_clears_desired_and_in_flight_but_retains_authority() {
    let controller = ModelPolicyController::new();
    let authoritative = base_policy();
    let desired = alternate_policy();
    let queued = policy("engine-q", "model-q", "low", "never", true);
    let failure = "persistence unavailable";

    controller.set_authoritative(authoritative.clone());
    controller.replace(desired.clone());
    let error = controller
        .flush(|in_flight| {
            assert_eq!(in_flight, desired);
            assert_eq!(controller.state().in_flight, Some(in_flight));
            controller.replace(queued.clone());
            Err::<SessionPolicy, _>(failure)
        })
        .expect_err("the injected persistence failure must escape");

    assert_eq!(error, ModelPolicyMutationError::new(failure));
    assert_eq!(error.cause(), &failure);
    assert_eq!(error.to_string(), failure);
    assert_eq!(error.clone().into_cause(), failure);
    let state = controller.state();
    assert_eq!(state.authoritative, Some(authoritative.clone()));
    assert_eq!(state.desired, None);
    assert_eq!(state.in_flight, None);
    assert_eq!(controller.current(), Some(authoritative));
}

#[test]
fn concurrent_flushes_are_serialized_while_first_flush_drains_queued_work() {
    let controller = Arc::new(ModelPolicyController::new());
    let first_policy = base_policy();
    let second_policy = alternate_policy();
    controller.replace(first_policy.clone());

    let (first_entered_tx, first_entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let first_controller = Arc::clone(&controller);
    let first = thread::spawn(move || {
        let mut persistence_calls = 0;
        first_controller
            .flush(|policy| {
                persistence_calls += 1;
                if persistence_calls == 1 {
                    first_entered_tx.send(()).expect("test receiver is live");
                    release_rx.recv().expect("test release is sent");
                }
                Ok::<_, &'static str>(policy)
            })
            .expect("first flush succeeds")
    });

    first_entered_rx
        .recv()
        .expect("first persistence invocation starts");

    let (second_started_tx, second_started_rx) = mpsc::channel();
    let second_controller = Arc::clone(&controller);
    let second = thread::spawn(move || {
        second_started_tx.send(()).expect("test receiver is live");
        second_controller
            .flush(Ok::<_, &'static str>)
            .expect("second flush succeeds after the first lock is released")
    });
    second_started_rx
        .recv()
        .expect("second flush thread starts");

    controller.replace(second_policy.clone());
    release_tx.send(()).expect("first callback is waiting");

    let first_result = first.join().expect("first flush thread joins");
    let second_result = second.join().expect("second flush thread joins");

    assert_flush_result(
        &first_result,
        &[first_policy.clone(), second_policy.clone()],
        Some(&second_policy),
    );
    assert_flush_result(&second_result, &[], Some(&second_policy));
}
