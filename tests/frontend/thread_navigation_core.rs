//! Exhaustive focused coverage for the dependency-free thread navigation core.
//!
//! The implementation is included directly so this harness does not require
//! shared `lib.rs`, BUILD, Cargo, or protocol registration.

#[path = "../../modules/frontend/src/thread_navigation_core.rs"]
mod thread_navigation_core;

use thread_navigation_core::{
    COMPLETE_STATUS, DETACHED_WORKSPACE_ROUTE_ID, FAILED_STATUS, IDLE_STATUS, LEGACY_FAILED_STATUS,
    LEGACY_THREAD_PREFIX, RESTING_THREAD_STATUSES, ThreadRouteOwner, ThreadRouteTarget,
    format_recent_thread_time, is_failed_status, is_resting_status, thread_completed,
    thread_failed, thread_has_active_work, thread_is_working, thread_route_id,
    thread_route_owns_target, thread_workspace_id, thread_workspace_route_id,
};

#[test]
fn route_id_normalization_strips_only_the_exact_prefix() {
    let cases = [
        ("plain", "plain"),
        ("thread_plain", "plain"),
        ("thread_thread_plain", "thread_plain"),
        ("thread_", "thread_"),
        ("", ""),
        ("Thread_plain", "Thread_plain"),
        ("xthread_plain", "xthread_plain"),
        ("thread", "thread"),
    ];

    assert_eq!(LEGACY_THREAD_PREFIX, "thread_");
    for (thread_id, expected) in cases {
        assert_eq!(
            thread_route_id(thread_id),
            expected,
            "thread_id={thread_id:?}"
        );
    }
}

#[test]
fn workspace_route_mapping_preserves_attached_values_and_reserves_underscore() {
    assert_eq!(thread_workspace_route_id(None), DETACHED_WORKSPACE_ROUTE_ID);
    assert_eq!(
        thread_workspace_route_id(Some("workspace-1")),
        "workspace-1"
    );
    assert_eq!(thread_workspace_route_id(Some("")), "");
    assert_eq!(
        thread_workspace_route_id(Some(DETACHED_WORKSPACE_ROUTE_ID)),
        "_"
    );

    assert_eq!(thread_workspace_id(DETACHED_WORKSPACE_ROUTE_ID), None);
    assert_eq!(thread_workspace_id("workspace-1"), Some("workspace-1"));
    assert_eq!(thread_workspace_id(""), Some(""));
}

#[test]
fn route_ownership_requires_exact_route_and_thread_identity() {
    let owner = ThreadRouteOwner::new(Some("/e/[workspace]/[thread]"), "thread-route");
    let matching = ThreadRouteTarget::new(Some("/e/[workspace]/[thread]"), Some("thread-route"));
    assert!(thread_route_owns_target(owner, matching));

    let cases = [
        (
            "different route surface",
            ThreadRouteTarget::new(Some("/t/[workspace]/[thread]"), Some("thread-route")),
        ),
        (
            "different thread parameter",
            ThreadRouteTarget::new(Some("/e/[workspace]/[thread]"), Some("other-thread")),
        ),
        (
            "missing thread parameter",
            ThreadRouteTarget::new(Some("/e/[workspace]/[thread]"), None),
        ),
        (
            "missing target route",
            ThreadRouteTarget::new(None, Some("thread-route")),
        ),
    ];
    for (label, target) in cases {
        assert!(!thread_route_owns_target(owner, target), "case={label}");
    }
}

#[test]
fn absent_route_surfaces_match_only_when_both_optional_route_ids_are_absent() {
    let owner = ThreadRouteOwner::new(None, "thread-route");
    assert!(thread_route_owns_target(
        owner,
        ThreadRouteTarget::new(None, Some("thread-route")),
    ));
    assert!(!thread_route_owns_target(
        owner,
        ThreadRouteTarget::new(Some("/e/[workspace]/[thread]"), Some("thread-route")),
    ));
}

#[test]
fn status_classification_is_exhaustive_for_resting_failure_and_working_values() {
    assert_eq!(RESTING_THREAD_STATUSES, [COMPLETE_STATUS, IDLE_STATUS]);

    let cases = [
        (COMPLETE_STATUS, true, false, false, true, false),
        (IDLE_STATUS, true, false, false, false, false),
        (FAILED_STATUS, false, true, true, false, false),
        (LEGACY_FAILED_STATUS, false, true, true, false, false),
        ("Waiting for answer", false, true, false, false, true),
        ("Running", false, true, false, false, true),
        ("unknown future status", false, true, false, false, true),
        (" complete", false, true, false, false, true),
    ];

    for (status, resting, working, failed, completed, active) in cases {
        assert_eq!(is_resting_status(status), resting, "status={status:?}");
        assert_eq!(thread_is_working(status), working, "status={status:?}");
        assert_eq!(is_failed_status(status), failed, "status={status:?}");
        assert_eq!(thread_failed(status), failed, "status={status:?}");
        assert_eq!(thread_completed(status), completed, "status={status:?}");
        assert_eq!(thread_has_active_work(status), active, "status={status:?}");
    }
}

#[test]
fn status_matching_is_exact_and_case_sensitive() {
    let lookalikes = [
        "complete",
        "COMPLETE",
        "Idle ",
        "failed to complete",
        "Needs attention ",
        "needs attention",
    ];

    for status in lookalikes {
        assert!(!is_resting_status(status), "status={status:?}");
        assert!(!is_failed_status(status), "status={status:?}");
        assert!(!thread_failed(status), "status={status:?}");
        assert!(!thread_completed(status), "status={status:?}");
        assert!(thread_is_working(status), "status={status:?}");
        assert!(thread_has_active_work(status), "status={status:?}");
    }
}

#[test]
fn recent_time_formatting_matches_every_boundary() {
    const NOW: i64 = 1_000_000_000;
    const MINUTE: i64 = 60_000;
    const HOUR: i64 = 3_600_000;
    const DAY: i64 = 86_400_000;

    let cases = [
        (0, "Just now"),
        (MINUTE - 1, "Just now"),
        (MINUTE, "1 min ago"),
        (MINUTE + 1, "1 min ago"),
        (59 * MINUTE + 59_999, "59 min ago"),
        (HOUR, "1 hr ago"),
        (HOUR + 1, "1 hr ago"),
        (23 * HOUR + 59 * MINUTE + 59_999, "23 hr ago"),
        (DAY, "Yesterday"),
        (2 * DAY - 1, "Yesterday"),
        (2 * DAY, "2 days ago"),
        (3 * DAY + 12 * HOUR, "3 days ago"),
    ];

    for (elapsed, expected) in cases {
        assert_eq!(
            format_recent_thread_time(NOW - elapsed, NOW),
            expected,
            "elapsed_ms={elapsed}"
        );
    }
}

#[test]
fn recent_time_formatting_uses_integer_units_and_clamps_negative_elapsed() {
    let now = 2_000_000_i64;
    assert_eq!(format_recent_thread_time(now, now), "Just now");
    assert_eq!(format_recent_thread_time(now + 59_999, now), "Just now");
    assert_eq!(format_recent_thread_time(now + 1, now), "Just now");
    assert_eq!(format_recent_thread_time(now - 125_999, now), "2 min ago");
    assert_eq!(format_recent_thread_time(now - 3_725_999, now), "1 hr ago");
}

#[test]
fn recent_time_formatting_handles_signed_extremes_without_overflow() {
    assert_eq!(
        format_recent_thread_time(i64::MAX, i64::MIN),
        "Just now",
        "future activity clamps the logically negative elapsed duration"
    );

    let very_old = format_recent_thread_time(i64::MIN, i64::MAX);
    assert_eq!(very_old, "213503982334 days ago");
}
