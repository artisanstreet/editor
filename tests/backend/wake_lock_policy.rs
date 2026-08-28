//! Exhaustive focused coverage for the dependency-free wake-lock policy.

#![forbid(unsafe_code)]

#[path = "../../modules/backend/src/wake_lock_policy.rs"]
mod wake_lock_policy;

use wake_lock_policy::{UnsettledWorkSnapshot, WakeLockAssessment, assess_unsettled_work};

const APPROVAL_GRACE_MS: i64 = 5 * 60_000;

fn snapshot<'a>(progressing_count: usize, approvals: &'a [i64]) -> UnsettledWorkSnapshot<'a> {
    UnsettledWorkSnapshot::new(progressing_count, approvals)
}

fn expected(held_count: u128, hold: bool, recheck_at_ms: Option<i128>) -> WakeLockAssessment {
    WakeLockAssessment {
        held_count,
        hold,
        recheck_at_ms,
    }
}

#[test]
fn zero_work_is_released_and_stable() {
    let assessment = assess_unsettled_work(snapshot(0, &[]), 1_000, APPROVAL_GRACE_MS);

    assert_eq!(assessment, expected(0, false, None));
}

#[test]
fn progressing_work_holds_without_a_timer_recheck() {
    let assessment = assess_unsettled_work(snapshot(3, &[]), 1_000, APPROVAL_GRACE_MS);

    assert_eq!(assessment, expected(3, true, None));
}

#[test]
fn a_fresh_approval_holds_until_its_strict_grace_expiry() {
    let requested_at_ms = 10_000;
    let assessment = assess_unsettled_work(
        snapshot(0, &[requested_at_ms]),
        requested_at_ms + 60_000,
        APPROVAL_GRACE_MS,
    );

    assert_eq!(
        assessment,
        expected(
            1,
            true,
            Some(i128::from(requested_at_ms) + i128::from(APPROVAL_GRACE_MS)),
        )
    );
}

#[test]
fn approval_grace_boundary_is_expired_at_equality_and_fresh_one_tick_before() {
    let requested_at_ms = 10_000;

    let one_before = assess_unsettled_work(
        snapshot(0, &[requested_at_ms]),
        requested_at_ms + APPROVAL_GRACE_MS - 1,
        APPROVAL_GRACE_MS,
    );
    assert_eq!(
        one_before,
        expected(
            1,
            true,
            Some(i128::from(requested_at_ms) + i128::from(APPROVAL_GRACE_MS)),
        )
    );

    let exact = assess_unsettled_work(
        snapshot(0, &[requested_at_ms]),
        requested_at_ms + APPROVAL_GRACE_MS,
        APPROVAL_GRACE_MS,
    );
    assert_eq!(exact, expected(0, false, None));

    let one_after = assess_unsettled_work(
        snapshot(0, &[requested_at_ms]),
        requested_at_ms + APPROVAL_GRACE_MS + 1,
        APPROVAL_GRACE_MS,
    );
    assert_eq!(one_after, expected(0, false, None));
}

#[test]
fn multiple_unordered_approvals_choose_the_earliest_graced_expiry() {
    let assessment = assess_unsettled_work(
        snapshot(1, &[40_000, 20_000, 30_000]),
        50_000,
        APPROVAL_GRACE_MS,
    );

    assert_eq!(
        assessment,
        expected(
            4,
            true,
            Some(i128::from(20_000) + i128::from(APPROVAL_GRACE_MS)),
        )
    );
}

#[test]
fn stale_approvals_do_not_become_the_recheck_when_other_approvals_are_fresh() {
    let assessment = assess_unsettled_work(snapshot(0, &[-1_000, 400, 200]), 500, 250);

    assert_eq!(assessment, expected(1, true, Some(650)));
}

#[test]
fn progressing_work_still_holds_when_every_approval_has_aged_out() {
    let assessment =
        assess_unsettled_work(snapshot(2, &[0]), APPROVAL_GRACE_MS * 2, APPROVAL_GRACE_MS);

    assert_eq!(assessment, expected(2, true, None));
}

#[test]
fn negative_and_future_relative_timestamps_use_signed_age_arithmetic() {
    let negative_epoch = assess_unsettled_work(snapshot(0, &[-10]), -5, 6);
    assert_eq!(negative_epoch, expected(1, true, Some(-4)));

    // The request is ten milliseconds in the future. Its age is -10, which
    // is strictly below the zero grace boundary and therefore still graced.
    let future_request = assess_unsettled_work(snapshot(0, &[10]), 0, 0);
    assert_eq!(future_request, expected(1, true, Some(10)));
}

#[test]
fn signed_grace_extremes_preserve_the_source_comparison() {
    // A negative grace remains representable at this pure boundary, so the
    // strict comparison can be checked even for a future-relative request.
    let assessment = assess_unsettled_work(snapshot(0, &[5, 10]), 0, -5);

    assert_eq!(assessment, expected(1, true, Some(5)));
}

#[test]
fn zero_grace_counts_only_future_requests() {
    let assessment = assess_unsettled_work(snapshot(0, &[-1, 0, 1]), 0, 0);

    assert_eq!(assessment, expected(1, true, Some(1)));
}

#[test]
fn timestamp_extremes_do_not_wrap_age_or_expiry_arithmetic() {
    let assessment = assess_unsettled_work(snapshot(0, &[i64::MAX]), i64::MIN, i64::MAX);

    assert_eq!(
        assessment,
        expected(1, true, Some(i128::from(i64::MAX) * 2))
    );

    let stale = assess_unsettled_work(snapshot(0, &[i64::MIN]), i64::MAX, i64::MAX);
    assert_eq!(stale, expected(0, false, None));
}

#[test]
fn held_count_does_not_wrap_at_the_usize_boundary() {
    let assessment = assess_unsettled_work(snapshot(usize::MAX, &[0]), 0, 1);

    assert_eq!(assessment, expected(usize::MAX as u128 + 1, true, Some(1)));
}

#[test]
fn an_empty_approval_set_is_stable_for_every_grace_sign() {
    for approval_grace_ms in [i64::MIN, -1, 0, 1, i64::MAX] {
        let assessment = assess_unsettled_work(snapshot(0, &[]), 0, approval_grace_ms);
        assert_eq!(
            assessment,
            expected(0, false, None),
            "grace={approval_grace_ms}"
        );
    }
}
