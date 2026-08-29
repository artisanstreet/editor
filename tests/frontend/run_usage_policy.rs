//! Dependency-free state-table coverage for the run-usage subscription policy.

#[path = "../../modules/frontend/src/run_usage_policy.rs"]
mod run_usage_policy;

use run_usage_policy::{
    RunUsageAction, RunUsageHostAction, RunUsagePolicy, RunUsageState, RunUsageSubscriptionRequest,
    RunUsageSubscriptionScope,
};

type Policy = RunUsagePolicy<u32>;

fn publish(state: RunUsageState<u32>) -> RunUsageAction<u32> {
    RunUsageAction::Publish(state)
}

fn interrupt(owner_id: u64, run_id: &str) -> RunUsageAction<u32> {
    RunUsageAction::Host(RunUsageHostAction::InterruptSubscription(
        RunUsageSubscriptionRequest::app(owner_id, run_id),
    ))
}

fn start(owner_id: u64, run_id: &str) -> RunUsageAction<u32> {
    RunUsageAction::Host(RunUsageHostAction::StartAuthoritativeSubscription(
        RunUsageSubscriptionRequest::app(owner_id, run_id),
    ))
}

#[test]
fn acquire_selects_an_initial_run_and_publishes_before_starting_it() {
    let mut policy = Policy::new();

    let lease = policy
        .acquire(Some("run-a"))
        .expect("first lease owner is representable");

    assert_eq!(lease.owner_id(), 1);
    assert_eq!(policy.active_owner_id(), Some(1));
    assert_eq!(policy.active_run_id(), Some("run-a"));
    assert_eq!(policy.state().run_id(), Some("run-a"));
    assert_eq!(policy.actions().len(), 2);
    assert_eq!(
        policy.take_actions(),
        vec![
            publish(RunUsageState::Loading {
                run_id: "run-a".to_owned(),
            }),
            start(1, "run-a"),
        ]
    );
    assert_eq!(
        policy.state(),
        &RunUsageState::Loading {
            run_id: "run-a".to_owned()
        }
    );
}

#[test]
fn owner_ids_are_monotonic_across_releases_and_undefined_acquires() {
    let mut policy: run_usage_policy::RunUsageController<u32> = RunUsagePolicy::default();

    let first = policy.acquire(None).expect("first owner");
    policy.take_actions();
    policy.release(first);
    policy.take_actions();

    let second = policy.acquire(None).expect("second owner");
    policy.take_actions();
    let third = policy.acquire(None).expect("third owner");

    assert_eq!(
        (first.owner_id(), second.owner_id(), third.owner_id()),
        (1, 2, 3)
    );
    assert_eq!(policy.active_owner_id(), Some(3));
    assert_eq!(policy.take_actions(), vec![publish(RunUsageState::None)]);
}

#[test]
fn same_owner_and_same_run_is_a_no_op_even_after_a_ready_update() {
    let mut policy = Policy::new();
    let lease = policy.acquire(Some("run-a")).expect("lease");
    policy.take_actions();

    policy.accept_update(lease.owner_id(), "run-a", 42);
    assert_eq!(
        policy.take_actions(),
        vec![publish(RunUsageState::Ready {
            aggregate: 42,
            run_id: "run-a".to_owned(),
        })]
    );

    policy.select(lease, Some("run-a"));

    assert!(policy.take_actions().is_empty());
    assert_eq!(
        policy.state(),
        &RunUsageState::Ready {
            aggregate: 42,
            run_id: "run-a".to_owned(),
        }
    );
}

#[test]
fn switching_runs_interrupts_old_then_publishes_loading_then_starts_new() {
    let mut policy = Policy::new();
    let lease = policy.acquire(Some("run-a")).expect("lease");
    policy.take_actions();

    policy.select_owner(lease.owner_id(), Some("run-b"));

    assert_eq!(
        policy.take_actions(),
        vec![
            interrupt(lease.owner_id(), "run-a"),
            publish(RunUsageState::Loading {
                run_id: "run-b".to_owned(),
            }),
            start(lease.owner_id(), "run-b"),
        ]
    );
    assert_eq!(policy.active_run_id(), Some("run-b"));
}

#[test]
fn deselection_interrupts_and_publishes_none_while_retaining_the_owner() {
    let mut policy = Policy::new();
    let lease = policy.acquire(Some("run-a")).expect("lease");
    policy.take_actions();

    policy.select(lease, None);

    assert_eq!(
        policy.take_actions(),
        vec![
            interrupt(lease.owner_id(), "run-a"),
            publish(RunUsageState::None)
        ]
    );
    assert_eq!(policy.active_owner_id(), Some(lease.owner_id()));
    assert_eq!(policy.active_run_id(), None);

    policy.select(lease, None);
    assert!(policy.take_actions().is_empty());
    policy.accept_update(lease.owner_id(), "run-a", 7);
    policy.accept_failure(lease.owner_id(), "run-a");
    assert!(policy.take_actions().is_empty());
    assert_eq!(policy.state(), &RunUsageState::None);
}

#[test]
fn updates_and_failures_require_both_owner_and_run_fences() {
    let mut policy = Policy::new();
    let lease = policy.acquire(Some("run-a")).expect("lease");
    policy.take_actions();

    policy.accept_update(lease.owner_id() + 1, "run-a", 1);
    policy.accept_update(lease.owner_id(), "run-b", 2);
    policy.accept_failure(lease.owner_id() + 1, "run-a");
    policy.accept_failure(lease.owner_id(), "run-b");
    assert!(policy.take_actions().is_empty());
    assert_eq!(
        policy.state(),
        &RunUsageState::Loading {
            run_id: "run-a".to_owned()
        }
    );

    policy.update(lease.owner_id(), "run-a", 3);
    assert_eq!(
        policy.take_actions(),
        vec![publish(RunUsageState::Ready {
            aggregate: 3,
            run_id: "run-a".to_owned(),
        })]
    );

    policy.failure(lease.owner_id(), "run-a");
    assert_eq!(
        policy.take_actions(),
        vec![publish(RunUsageState::Unavailable {
            run_id: "run-a".to_owned(),
        })]
    );
}

#[test]
fn stale_subscription_events_after_a_switch_cannot_repopulate_the_view() {
    let mut policy = Policy::new();
    let old = policy.acquire(Some("run-a")).expect("old lease");
    policy.take_actions();
    let current = policy.acquire(Some("run-b")).expect("current lease");
    policy.take_actions();

    policy.accept_update(old.owner_id(), "run-a", 10);
    policy.accept_failure(old.owner_id(), "run-a");
    assert!(policy.take_actions().is_empty());
    assert_eq!(
        policy.state(),
        &RunUsageState::Loading {
            run_id: "run-b".to_owned()
        }
    );

    policy.accept_update(current.owner_id(), "run-b", 20);
    assert_eq!(
        policy.take_actions(),
        vec![publish(RunUsageState::Ready {
            aggregate: 20,
            run_id: "run-b".to_owned(),
        })]
    );
}

#[test]
fn matching_release_interrupts_and_stale_release_is_a_no_op() {
    let mut policy = Policy::new();
    let old = policy.acquire(Some("run-a")).expect("old lease");
    policy.take_actions();
    let current = policy.acquire(Some("run-b")).expect("current lease");
    policy.take_actions();

    policy.release(old);
    assert!(policy.take_actions().is_empty());
    assert_eq!(policy.active_owner_id(), Some(current.owner_id()));
    assert_eq!(policy.active_run_id(), Some("run-b"));

    policy.release(current);
    assert_eq!(
        policy.take_actions(),
        vec![
            interrupt(current.owner_id(), "run-b"),
            publish(RunUsageState::None)
        ]
    );
    assert_eq!(policy.active_owner_id(), None);
    assert_eq!(policy.state(), &RunUsageState::None);

    policy.release(current);
    assert!(policy.take_actions().is_empty());
}

#[test]
fn an_old_lease_can_reselect_and_replace_a_newer_owner_like_the_legacy_controller() {
    let mut policy = Policy::new();
    let old = policy.acquire(Some("run-a")).expect("old lease");
    policy.take_actions();
    let newer = policy.acquire(Some("run-b")).expect("newer lease");
    policy.take_actions();

    policy.select(old, Some("run-a"));

    assert_eq!(
        policy.take_actions(),
        vec![
            interrupt(newer.owner_id(), "run-b"),
            publish(RunUsageState::Loading {
                run_id: "run-a".to_owned(),
            }),
            start(old.owner_id(), "run-a"),
        ]
    );
    assert_eq!(policy.active_owner_id(), Some(old.owner_id()));

    policy.accept_update(newer.owner_id(), "run-b", 99);
    assert!(policy.take_actions().is_empty());
    policy.accept_update(old.owner_id(), "run-a", 11);
    assert_eq!(
        policy.take_actions(),
        vec![publish(RunUsageState::Ready {
            aggregate: 11,
            run_id: "run-a".to_owned(),
        })]
    );
}

#[test]
fn subscription_host_actions_are_explicitly_application_owned() {
    let mut policy = Policy::new();
    let lease = policy.acquire(Some("run-a")).expect("lease");
    let actions = policy.take_actions();

    for action in actions {
        if let RunUsageAction::Host(host_action) = action {
            assert_eq!(host_action.request().scope, RunUsageSubscriptionScope::App);
            assert_eq!(host_action.request().scope.as_str(), "app");
            assert_eq!(host_action.request().owner_id, lease.owner_id());
            assert_eq!(host_action.request().run_id, "run-a");
        }
    }

    policy.release(lease);
    let release_actions = policy.take_actions();
    assert!(matches!(
        release_actions.first(),
        Some(RunUsageAction::Host(RunUsageHostAction::InterruptSubscription(request)))
            if request.scope == RunUsageSubscriptionScope::App
                && request.owner_id == lease.owner_id()
                && request.run_id == "run-a"
    ));
}
