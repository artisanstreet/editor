//! Focused, dependency-free tests for SQLite write-retry classification.

#![allow(dead_code)]

#[path = "../../modules/backend/src/sqlite_write_retry_policy.rs"]
mod sqlite_write_retry_policy;

use std::time::Duration;

use sqlite_write_retry_policy::{
    CauseObservation, CauseReasonObservation, INITIAL_RETRY_DELAY, MAX_RETRY_DELAY,
    MAX_RETRY_REPETITIONS, SqliteWriteErrorObservation, SqliteWriteRetryDecision,
    SqliteWriteRetryPolicy, exponential_retry_delay, retry_delay_for, sqlite_write_retry_decision,
};

fn direct_sql(retryable: bool) -> SqliteWriteErrorObservation {
    SqliteWriteErrorObservation::direct_sql(retryable)
}

fn recognized(reasons: Vec<CauseReasonObservation>) -> SqliteWriteErrorObservation {
    SqliteWriteErrorObservation::drizzle_query(CauseObservation::recognized(reasons))
}

fn fail(error: SqliteWriteErrorObservation) -> CauseReasonObservation {
    CauseReasonObservation::fail(error)
}

#[test]
fn direct_sql_uses_only_its_retryable_flag() {
    assert!(direct_sql(true).is_retryable());
    assert!(!direct_sql(false).is_retryable());

    assert_eq!(
        sqlite_write_retry_decision(&direct_sql(true), 0),
        SqliteWriteRetryDecision::Retry {
            repetition: 0,
            delay: INITIAL_RETRY_DELAY,
        }
    );
    assert_eq!(
        sqlite_write_retry_decision(&direct_sql(false), 0),
        SqliteWriteRetryDecision::NotRetryable
    );
}

#[test]
fn every_top_level_unrecognized_or_ordinary_shape_is_not_retryable() {
    let observations = [
        SqliteWriteErrorObservation::unrecognized_wrapper(),
        SqliteWriteErrorObservation::ordinary(),
        SqliteWriteErrorObservation::drizzle_query(CauseObservation::unrecognized()),
    ];

    for observation in observations {
        assert!(!observation.is_retryable());
        assert_eq!(
            sqlite_write_retry_decision(&observation, 0),
            SqliteWriteRetryDecision::NotRetryable
        );
    }
}

#[test]
fn recognized_empty_cause_has_no_retryable_reason() {
    let observation = recognized(Vec::new());

    assert_eq!(
        observation,
        SqliteWriteErrorObservation::DrizzleQuery {
            cause: CauseObservation::Recognized {
                reasons: Vec::new()
            },
        }
    );
    assert!(!observation.is_retryable());
}

#[test]
fn fail_reason_recurses_through_nested_drizzle_queries() {
    let observation = recognized(vec![fail(recognized(vec![fail(recognized(vec![fail(
        direct_sql(true),
    )]))]))]);

    assert!(observation.is_retryable());
    assert_eq!(
        sqlite_write_retry_decision(&observation, 2),
        SqliteWriteRetryDecision::Retry {
            repetition: 2,
            delay: Duration::from_millis(20),
        }
    );
}

#[test]
fn nested_non_retryable_direct_sql_stays_non_retryable() {
    let observation = recognized(vec![fail(recognized(vec![fail(direct_sql(false))]))]);

    assert!(!observation.is_retryable());
    assert_eq!(
        sqlite_write_retry_decision(&observation, 0),
        SqliteWriteRetryDecision::NotRetryable
    );
}

#[test]
fn any_retryable_fail_reason_is_sufficient() {
    let observation = recognized(vec![
        fail(direct_sql(false)),
        CauseReasonObservation::defect(),
        CauseReasonObservation::interrupt(),
        fail(recognized(vec![fail(direct_sql(true))])),
        CauseReasonObservation::unrecognized(),
    ]);

    assert!(observation.is_retryable());
    assert_eq!(
        sqlite_write_retry_decision(&observation, 0),
        SqliteWriteRetryDecision::Retry {
            repetition: 0,
            delay: Duration::from_millis(5),
        }
    );
}

#[test]
fn non_fail_reasons_never_recurse_into_or_admit_nested_errors() {
    let observations = [
        recognized(vec![CauseReasonObservation::defect()]),
        recognized(vec![CauseReasonObservation::interrupt()]),
        recognized(vec![CauseReasonObservation::unrecognized()]),
    ];

    for observation in observations {
        assert!(!observation.is_retryable());
    }
}

#[test]
fn fail_reasons_with_unrecognized_or_ordinary_nested_errors_are_excluded() {
    let observations = [
        recognized(vec![fail(
            SqliteWriteErrorObservation::unrecognized_wrapper(),
        )]),
        recognized(vec![fail(SqliteWriteErrorObservation::ordinary())]),
        recognized(vec![fail(SqliteWriteErrorObservation::drizzle_query(
            CauseObservation::unrecognized(),
        ))]),
        recognized(vec![fail(SqliteWriteErrorObservation::drizzle_query(
            CauseObservation::recognized(Vec::new()),
        ))]),
    ];

    for observation in observations {
        assert!(!observation.is_retryable());
    }
}

#[test]
fn every_schedule_repetition_has_the_exact_exponential_delay() {
    let expected = [5_u64, 10, 20, 40, 80, 160, 320, 640];

    assert_eq!(INITIAL_RETRY_DELAY, Duration::from_millis(5));
    assert_eq!(MAX_RETRY_DELAY, Duration::from_secs(1));
    assert_eq!(MAX_RETRY_REPETITIONS, 8);

    for (repetition, milliseconds) in (0_u32..).zip(expected) {
        let expected_delay = Duration::from_millis(milliseconds);
        assert_eq!(exponential_retry_delay(repetition), expected_delay);
        assert_eq!(retry_delay_for(repetition), Some(expected_delay));
    }
}

#[test]
fn exponential_delay_caps_at_one_second_without_overflow() {
    assert_eq!(exponential_retry_delay(8), MAX_RETRY_DELAY);
    assert_eq!(exponential_retry_delay(9), MAX_RETRY_DELAY);
    assert_eq!(exponential_retry_delay(u32::MAX), MAX_RETRY_DELAY);
}

#[test]
fn schedule_boundary_exhausts_after_eight_repetitions() {
    let observation = direct_sql(true);

    assert_eq!(
        retry_delay_for(MAX_RETRY_REPETITIONS - 1),
        Some(Duration::from_millis(640))
    );
    assert_eq!(retry_delay_for(MAX_RETRY_REPETITIONS), None);
    assert_eq!(retry_delay_for(u32::MAX), None);

    assert_eq!(
        sqlite_write_retry_decision(&observation, MAX_RETRY_REPETITIONS - 1),
        SqliteWriteRetryDecision::Retry {
            repetition: 7,
            delay: Duration::from_millis(640),
        }
    );
    assert_eq!(
        sqlite_write_retry_decision(&observation, MAX_RETRY_REPETITIONS),
        SqliteWriteRetryDecision::Exhausted
    );
}

#[test]
fn non_retryable_error_wins_over_an_already_exhausted_schedule() {
    assert_eq!(
        sqlite_write_retry_decision(&direct_sql(false), MAX_RETRY_REPETITIONS),
        SqliteWriteRetryDecision::NotRetryable
    );
}

#[test]
fn decision_accessors_expose_only_retry_data_for_retry_decisions() {
    let retry = SqliteWriteRetryDecision::Retry {
        repetition: 3,
        delay: Duration::from_millis(40),
    };
    assert!(retry.should_retry());
    assert_eq!(retry.delay(), Some(Duration::from_millis(40)));
    assert_eq!(retry.repetition(), Some(3));

    for decision in [
        SqliteWriteRetryDecision::NotRetryable,
        SqliteWriteRetryDecision::Exhausted,
    ] {
        assert!(!decision.should_retry());
        assert_eq!(decision.delay(), None);
        assert_eq!(decision.repetition(), None);
    }
}

#[test]
fn policy_marker_is_stateless_and_repeated_decisions_are_independent() {
    let policy = SqliteWriteRetryPolicy::new();
    assert_eq!(policy, SqliteWriteRetryPolicy);

    let observation = recognized(vec![
        fail(direct_sql(false)),
        fail(recognized(vec![fail(direct_sql(true))])),
    ]);
    let first = SqliteWriteRetryPolicy::decide(&observation, 4);
    let second = SqliteWriteRetryPolicy::decide(&observation, 4);
    let free_function = sqlite_write_retry_decision(&observation, 4);

    assert_eq!(first, second);
    assert_eq!(first, free_function);
    assert_eq!(
        first,
        SqliteWriteRetryDecision::Retry {
            repetition: 4,
            delay: Duration::from_millis(80),
        }
    );
}
