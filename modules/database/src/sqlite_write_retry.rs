//! Bounded retry policy for SQLite writer-lock contention.
//!
//! The file-backed pool runs SQLite in WAL mode with `synchronous=NORMAL`.
//! A deferred transaction that reads first and writes later can fail with
//! `SQLITE_BUSY_SNAPSHOT` when another connection commits in between, and
//! `busy_timeout` does not cover that snapshot upgrade. Read-then-write
//! repository transactions therefore begin with `BEGIN IMMEDIATE` (see
//! [`crate::repository`]), and the writer fence itself is retried here on a
//! bounded exponential schedule. This module is the one production retry
//! schedule: five milliseconds doubling to a one-second cap over exactly
//! eight repetitions.

#![forbid(unsafe_code)]

use std::time::Duration;

use sea_orm::sqlx;
use sea_orm::{DbErr, RuntimeErr};

/// The initial delay before the first scheduled SQLite write retry.
pub const INITIAL_RETRY_DELAY: Duration = Duration::from_millis(INITIAL_RETRY_DELAY_MILLIS);

/// The largest delay produced by the bounded retry schedule.
pub const MAX_RETRY_DELAY: Duration = Duration::from_millis(MAX_RETRY_DELAY_MILLIS);

/// The number of retry repetitions admitted by the bounded schedule.
pub const MAX_RETRY_REPETITIONS: u32 = 8;

const INITIAL_RETRY_DELAY_MILLIS: u64 = 5;
const MAX_RETRY_DELAY_MILLIS: u64 = 1_000;

/// SQLite primary result code for `SQLITE_BUSY`.
const SQLITE_BUSY_PRIMARY: i32 = 5;

/// SQLite primary result code for `SQLITE_LOCKED`.
const SQLITE_LOCKED_PRIMARY: i32 = 6;

/// The deterministic decision for one failure at one retry boundary.
#[must_use = "a retry decision must be enforced at the transaction boundary"]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SqliteWriteRetryDecision {
    /// Retry the complete operation after the supplied bounded delay.
    Retry {
        /// The zero-based repetition being scheduled.
        repetition: u32,
        /// The delay before that repetition.
        delay: Duration,
    },
    /// The failure is not writer-lock contention; retrying cannot help.
    NotRetryable,
    /// Every bounded schedule repetition was already consumed.
    Exhausted,
}

impl SqliteWriteRetryDecision {
    /// Returns whether this decision admits another operation attempt.
    #[must_use]
    pub const fn should_retry(self) -> bool {
        matches!(self, Self::Retry { .. })
    }

    /// Returns the scheduled delay, if another attempt is admitted.
    #[must_use]
    pub const fn delay(self) -> Option<Duration> {
        match self {
            Self::Retry { delay, .. } => Some(delay),
            Self::NotRetryable | Self::Exhausted => None,
        }
    }

    /// Returns the scheduled repetition, if another attempt is admitted.
    #[must_use]
    pub const fn repetition(self) -> Option<u32> {
        match self {
            Self::Retry { repetition, .. } => Some(repetition),
            Self::NotRetryable | Self::Exhausted => None,
        }
    }
}

/// Returns the capped exponential delay for a zero-based schedule repetition.
///
/// Repetition zero is five milliseconds. Each following repetition doubles
/// the delay until the one-second cap. The calculation remains bounded even
/// for a repetition value greater than the schedule admits.
#[must_use]
pub const fn exponential_retry_delay(repetition: u32) -> Duration {
    let exponent = if repetition > 8 { 8 } else { repetition };
    let uncapped_milliseconds = INITIAL_RETRY_DELAY_MILLIS << exponent;
    let milliseconds = if uncapped_milliseconds > MAX_RETRY_DELAY_MILLIS {
        MAX_RETRY_DELAY_MILLIS
    } else {
        uncapped_milliseconds
    };
    Duration::from_millis(milliseconds)
}

/// Returns the delay for an admitted retry repetition.
///
/// The schedule admits exactly [`MAX_RETRY_REPETITIONS`] repetitions, indexed
/// from zero. Once that bound is reached, no delay is scheduled and `None` is
/// returned.
#[must_use]
pub const fn retry_delay_for(repetition: u32) -> Option<Duration> {
    if repetition < MAX_RETRY_REPETITIONS {
        Some(exponential_retry_delay(repetition))
    } else {
        None
    }
}

/// Classifies one `SeaORM` failure as SQLite writer-lock contention.
///
/// The extended driver result code is reduced to SQLite's primary code, so
/// every `SQLITE_BUSY` shape (including `SQLITE_BUSY_SNAPSHOT`) and every
/// `SQLITE_LOCKED` shape is retryable while every other failure is not.
#[must_use]
pub fn is_retryable_write_error(error: &DbErr) -> bool {
    sqlite_primary_code(error)
        .is_some_and(|code| code == SQLITE_BUSY_PRIMARY || code == SQLITE_LOCKED_PRIMARY)
}

/// Decides one retry boundary from a concrete database failure.
///
/// A retryable writer-lock failure is still [`SqliteWriteRetryDecision::Exhausted`]
/// once the exact eight-repetition schedule bound is reached.
pub fn decide_write_retry(error: &DbErr, completed_repetitions: u32) -> SqliteWriteRetryDecision {
    if !is_retryable_write_error(error) {
        return SqliteWriteRetryDecision::NotRetryable;
    }
    retry_delay_for(completed_repetitions).map_or(SqliteWriteRetryDecision::Exhausted, |delay| {
        SqliteWriteRetryDecision::Retry {
            repetition: completed_repetitions,
            delay,
        }
    })
}

/// Reduces a `SeaORM`/SQLite driver failure to its primary result code.
fn sqlite_primary_code(error: &DbErr) -> Option<i32> {
    let (DbErr::Exec(RuntimeErr::SqlxError(sqlx_error))
    | DbErr::Query(RuntimeErr::SqlxError(sqlx_error))
    | DbErr::Conn(RuntimeErr::SqlxError(sqlx_error))) = error
    else {
        return None;
    };
    let sqlx::Error::Database(database_error) = &**sqlx_error else {
        return None;
    };
    database_error
        .code()?
        .parse::<i32>()
        .ok()
        .map(|code| code & 0xff)
}

/// Repeatedly invokes `operation` while it fails on SQLite writer contention.
///
/// The first success, the first non-retryable failure, and the failure after
/// the bounded schedule is exhausted are all returned unchanged; only
/// writer-lock failures consume schedule repetitions, and each consumes one
/// bounded sleep.
pub(crate) async fn retry_write_operation<T, F, Fut>(mut operation: F) -> Result<T, DbErr>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, DbErr>>,
{
    let mut completed_repetitions = 0_u32;
    loop {
        match operation().await {
            Ok(value) => return Ok(value),
            Err(source) => match decide_write_retry(&source, completed_repetitions) {
                SqliteWriteRetryDecision::Retry { delay, .. } => {
                    tokio::time::sleep(delay).await;
                    completed_repetitions += 1;
                }
                SqliteWriteRetryDecision::NotRetryable | SqliteWriteRetryDecision::Exhausted => {
                    return Err(source);
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::cell::Cell;
    use std::error::Error as StdError;
    use std::sync::Arc;

    use sea_orm::sqlx::error::{DatabaseError, ErrorKind};

    use super::*;

    #[derive(Debug)]
    struct FakeDatabaseError {
        code: i32,
    }

    impl std::fmt::Display for FakeDatabaseError {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(formatter, "(code: {}) database is locked", self.code)
        }
    }

    impl StdError for FakeDatabaseError {}

    impl DatabaseError for FakeDatabaseError {
        fn message(&self) -> &'static str {
            "database is locked"
        }

        fn code(&self) -> Option<Cow<'_, str>> {
            Some(Cow::Owned(self.code.to_string()))
        }

        fn as_error(&self) -> &(dyn StdError + Send + Sync + 'static) {
            self
        }

        fn as_error_mut(&mut self) -> &mut (dyn StdError + Send + Sync + 'static) {
            self
        }

        fn into_error(self: Box<Self>) -> Box<dyn StdError + Send + Sync + 'static> {
            self
        }

        fn kind(&self) -> ErrorKind {
            ErrorKind::Other
        }
    }

    fn driver_error(code: i32) -> DbErr {
        DbErr::Query(RuntimeErr::SqlxError(Arc::new(sqlx::Error::Database(
            Box::new(FakeDatabaseError { code }),
        ))))
    }

    fn busy_snapshot() -> DbErr {
        driver_error(517)
    }

    fn permanent_failure() -> DbErr {
        DbErr::Query(RuntimeErr::Internal("syntax error".to_owned()))
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
        assert_eq!(exponential_retry_delay(8), MAX_RETRY_DELAY);
        assert_eq!(exponential_retry_delay(u32::MAX), MAX_RETRY_DELAY);
        assert_eq!(retry_delay_for(MAX_RETRY_REPETITIONS), None);
        assert_eq!(retry_delay_for(u32::MAX), None);
    }

    #[test]
    fn every_busy_and_locked_extended_code_is_retryable() {
        // SQLITE_BUSY, SQLITE_BUSY_RECOVERY, SQLITE_BUSY_SNAPSHOT,
        // SQLITE_BUSY_TIMEOUT, SQLITE_LOCKED, and LOCKED_SHAREDCACHE.
        for code in [5, 261, 517, 773, 6, 262] {
            let error = driver_error(code);
            assert!(
                is_retryable_write_error(&error),
                "extended code {code} must be retryable"
            );
            assert_eq!(
                decide_write_retry(&error, 0),
                SqliteWriteRetryDecision::Retry {
                    repetition: 0,
                    delay: INITIAL_RETRY_DELAY,
                }
            );
        }
    }

    #[test]
    fn non_contention_and_non_driver_failures_are_not_retryable() {
        // SQLITE_CONSTRAINT, SQLITE_CORRUPT, and a driver-independent error.
        for error in [driver_error(19), driver_error(11), permanent_failure()] {
            assert!(!is_retryable_write_error(&error));
            assert_eq!(
                decide_write_retry(&error, 0),
                SqliteWriteRetryDecision::NotRetryable
            );
        }
    }

    #[test]
    fn exhaustion_wins_after_the_exact_schedule_bound() {
        let error = busy_snapshot();

        assert_eq!(
            decide_write_retry(&error, MAX_RETRY_REPETITIONS - 1),
            SqliteWriteRetryDecision::Retry {
                repetition: 7,
                delay: Duration::from_millis(640),
            }
        );
        assert_eq!(
            decide_write_retry(&error, MAX_RETRY_REPETITIONS),
            SqliteWriteRetryDecision::Exhausted
        );
        assert_eq!(
            decide_write_retry(&permanent_failure(), MAX_RETRY_REPETITIONS),
            SqliteWriteRetryDecision::NotRetryable
        );
    }

    #[test]
    fn decision_accessors_expose_only_admitted_retry_data() {
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

    #[tokio::test]
    async fn retry_write_operation_reissues_retryable_contention() {
        let busy = busy_snapshot();
        let mut planned = vec![Err(busy.clone()), Err(busy), Ok(7_u8)];
        let attempts = Cell::new(0_usize);

        let result = retry_write_operation(|| {
            attempts.set(attempts.get() + 1);
            std::future::ready(planned.remove(0))
        })
        .await;

        assert_eq!(result, Ok(7));
        assert_eq!(attempts.get(), 3);
    }

    #[tokio::test]
    async fn retry_write_operation_surfaces_permanent_failures_immediately() {
        let attempts = Cell::new(0_usize);

        let result = retry_write_operation(|| {
            attempts.set(attempts.get() + 1);
            std::future::ready(Err::<u8, _>(permanent_failure()))
        })
        .await;

        assert!(result.is_err());
        assert_eq!(attempts.get(), 1);
    }

    #[tokio::test]
    async fn retry_write_operation_surfaces_exhausted_contention() {
        let busy = busy_snapshot();
        let attempts = Cell::new(0_usize);

        let result = retry_write_operation(|| {
            attempts.set(attempts.get() + 1);
            std::future::ready(Err::<u8, _>(busy.clone()))
        })
        .await;

        assert!(result.is_err());
        assert_eq!(
            attempts.get(),
            usize::try_from(MAX_RETRY_REPETITIONS).expect("bound fits usize") + 1
        );
    }
}
