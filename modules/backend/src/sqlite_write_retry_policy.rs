//! Dependency-free SQLite write-retry classification and schedule policy.
//!
//! The TypeScript persistence boundary owns the concrete Effect SQL and
//! Drizzle error values. This module receives only typed observations of
//! those values and returns deterministic decisions. It never sleeps,
//! executes an operation, performs database I/O, or depends on either
//! JavaScript library.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use std::time::Duration;

const INITIAL_RETRY_DELAY_MILLIS: u64 = 5;
const MAX_RETRY_DELAY_MILLIS: u64 = 1_000;

/// The initial delay for the first scheduled SQLite write retry.
pub const INITIAL_RETRY_DELAY: Duration = Duration::from_millis(INITIAL_RETRY_DELAY_MILLIS);

/// The largest delay produced by the exponential retry schedule.
pub const MAX_RETRY_DELAY: Duration = Duration::from_millis(MAX_RETRY_DELAY_MILLIS);

/// The number of retry repetitions admitted by the bounded schedule.
pub const MAX_RETRY_REPETITIONS: u32 = 8;

/// An observation supplied by the concrete database boundary.
///
/// The variants deliberately distinguish the direct SQL error recognized by
/// Effect, the Drizzle query wrapper recognized by the persistence seam, and
/// values that do not match either recognized shape. Classification never
/// guesses from an ordinary or unrecognized value.
#[must_use = "an error observation must be classified before deciding whether to retry"]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SqliteWriteErrorObservation {
    /// A direct Effect SQL error and its exact retryable flag.
    DirectSql {
        /// The flag supplied by the direct SQL error.
        retryable: bool,
    },
    /// A recognized Drizzle query error and its supplied Effect cause.
    DrizzleQuery {
        /// The cause shape owned by the Drizzle query error.
        cause: CauseObservation,
    },
    /// A wrapper that is not a recognized direct SQL or Drizzle error.
    UnrecognizedWrapper,
    /// An ordinary unrelated error.
    Ordinary,
}

impl SqliteWriteErrorObservation {
    /// Creates a direct SQL observation with the exact supplied flag.
    pub const fn direct_sql(retryable: bool) -> Self {
        Self::DirectSql { retryable }
    }

    /// Creates a recognized Drizzle query observation.
    pub const fn drizzle_query(cause: CauseObservation) -> Self {
        Self::DrizzleQuery { cause }
    }

    /// Creates an observation for an unrecognized wrapper.
    pub const fn unrecognized_wrapper() -> Self {
        Self::UnrecognizedWrapper
    }

    /// Creates an observation for an ordinary unrelated error.
    pub const fn ordinary() -> Self {
        Self::Ordinary
    }

    /// Returns whether this observation is retryable under the SQL seam.
    ///
    /// Direct SQL observations use only their own flag. Drizzle observations
    /// require a recognized cause and at least one fail reason whose nested
    /// observation is recursively retryable. Every other shape is false.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::DirectSql { retryable } => *retryable,
            Self::DrizzleQuery {
                cause: CauseObservation::Recognized { reasons },
            } => reasons
                .iter()
                .any(CauseReasonObservation::contains_retryable_failure),
            Self::DrizzleQuery {
                cause: CauseObservation::Unrecognized,
            }
            | Self::UnrecognizedWrapper
            | Self::Ordinary => false,
        }
    }
}

/// A supplied observation of the Effect `Cause` shape owned by a Drizzle
/// query error.
///
/// `Recognized` corresponds to `Cause.isCause(...)` succeeding. An empty
/// recognized cause is valid but cannot contain a retryable fail reason.
#[must_use]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CauseObservation {
    /// A recognized cause with its ordered reasons.
    Recognized {
        /// The cause reasons supplied by the Effect boundary.
        reasons: Vec<CauseReasonObservation>,
    },
    /// A value that was supplied as a cause but failed `Cause.isCause(...)`.
    Unrecognized,
}

impl CauseObservation {
    /// Creates a recognized cause from its supplied reasons.
    pub fn recognized(reasons: Vec<CauseReasonObservation>) -> Self {
        Self::Recognized { reasons }
    }

    /// Creates an unrecognized cause observation.
    pub const fn unrecognized() -> Self {
        Self::Unrecognized
    }
}

/// One reason supplied by a recognized Effect cause.
///
/// Only `Fail` reasons participate in Drizzle retry classification. Defects,
/// interruptions, and unrecognized reasons are retained as typed observations
/// so they cannot accidentally be treated as failures.
#[must_use]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CauseReasonObservation {
    /// A failure reason containing a recursively inspectable error.
    Fail {
        /// The nested error carried by the fail reason.
        error: Box<SqliteWriteErrorObservation>,
    },
    /// An Effect defect/non-fail reason.
    Defect,
    /// An Effect interruption/non-fail reason.
    Interrupt,
    /// A reason that is not recognized as any supported cause reason.
    Unrecognized,
}

impl CauseReasonObservation {
    /// Creates a fail reason around a nested error observation.
    pub fn fail(error: SqliteWriteErrorObservation) -> Self {
        Self::Fail {
            error: Box::new(error),
        }
    }

    /// Creates a defect reason.
    pub const fn defect() -> Self {
        Self::Defect
    }

    /// Creates an interruption reason.
    pub const fn interrupt() -> Self {
        Self::Interrupt
    }

    /// Creates an unrecognized non-fail reason.
    pub const fn unrecognized() -> Self {
        Self::Unrecognized
    }

    fn contains_retryable_failure(&self) -> bool {
        match self {
            Self::Fail { error } => error.is_retryable(),
            Self::Defect | Self::Interrupt | Self::Unrecognized => false,
        }
    }
}

/// Returns the capped exponential delay for a zero-based schedule repetition.
///
/// Repetition zero is five milliseconds. Each following repetition doubles
/// the delay until the one-second cap. The calculation remains bounded even
/// for a repetition value greater than the retry schedule admits.
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

/// The deterministic result of classifying one failure at one retry boundary.
#[must_use = "a retry decision must be enforced at the transaction boundary"]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SqliteWriteRetryDecision {
    /// Retry the complete operation after the supplied delay.
    Retry {
        /// The zero-based repetition being scheduled.
        repetition: u32,
        /// The delay before that repetition.
        delay: Duration,
    },
    /// Do not retry because the error is not retryable.
    NotRetryable,
    /// Do not retry because all bounded schedule repetitions were used.
    Exhausted,
}

impl SqliteWriteRetryDecision {
    /// Returns whether this decision schedules another operation attempt.
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

/// Stateless entry point for the SQLite write-retry policy.
#[must_use]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SqliteWriteRetryPolicy;

impl SqliteWriteRetryPolicy {
    /// Creates the stateless policy marker.
    pub const fn new() -> Self {
        Self
    }

    /// Decides whether the complete operation may be retried.
    ///
    /// `completed_repetitions` is the number of schedule repetitions already
    /// consumed. A value of zero therefore asks for the first five-millisecond
    /// retry delay. The supplied observation is borrowed and never changed.
    #[must_use = "a retry decision must be enforced at the transaction boundary"]
    pub fn decide(
        observation: &SqliteWriteErrorObservation,
        completed_repetitions: u32,
    ) -> SqliteWriteRetryDecision {
        sqlite_write_retry_decision(observation, completed_repetitions)
    }
}

/// Classifies an observed SQLite write error and returns its retry decision.
///
/// This function is pure: it does not execute a closure, sleep, or perform
/// database I/O. A retryable observation is still exhausted once the exact
/// eight-repetition schedule bound has been reached.
#[must_use = "a retry decision must be enforced at the transaction boundary"]
pub fn sqlite_write_retry_decision(
    observation: &SqliteWriteErrorObservation,
    completed_repetitions: u32,
) -> SqliteWriteRetryDecision {
    if !observation.is_retryable() {
        return SqliteWriteRetryDecision::NotRetryable;
    }

    match retry_delay_for(completed_repetitions) {
        Some(delay) => SqliteWriteRetryDecision::Retry {
            repetition: completed_repetitions,
            delay,
        },
        None => SqliteWriteRetryDecision::Exhausted,
    }
}
