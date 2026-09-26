//! Age groups of the sidebar's recent threads.
//!
//! The Forge decides which threads are recent, orders them newest activity
//! first, and resolves each row's subtitle; the Editor only sorts the rows
//! it received into age groups for presentation, as a pure function of the
//! listing and the clock. Groups keep the Forge's order, and empty groups
//! are not shown. As time passes a row moves to an older group; the next
//! instant any row crosses a boundary is [`next_regrouping`].

#![forbid(unsafe_code)]

use artisan_domain::{RecentThread, RecentThreadListing, UnixMillis};

const HOUR_MS: i64 = 60 * 60 * 1_000;
const DAY_MS: i64 = 24 * HOUR_MS;

/// One age group of recent threads, youngest first.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum RecentThreadAge {
    /// Active less than 24 hours ago (or, with a skewed clock, in the future).
    LastDay,
    /// Active 24 hours to less than 3 days ago.
    LastThreeDays,
    /// Active 3 days to less than 30 days ago.
    LastMonth,
    /// Active 30 days ago or earlier.
    Older,
}

impl RecentThreadAge {
    /// Every group, youngest first.
    pub const ALL: [Self; 4] = [
        Self::LastDay,
        Self::LastThreeDays,
        Self::LastMonth,
        Self::Older,
    ];

    /// The group heading.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::LastDay => "Last 24 hours",
            Self::LastThreeDays => "Last 3 days",
            Self::LastMonth => "Last 30 days",
            Self::Older => "Older",
        }
    }

    /// A stable identifier for element ids and selectors.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::LastDay => "last-24-hours",
            Self::LastThreeDays => "last-3-days",
            Self::LastMonth => "last-30-days",
            Self::Older => "older",
        }
    }

    /// The age at which a row leaves this group, if it ever does.
    const fn upper_bound_ms(self) -> Option<i64> {
        match self {
            Self::LastDay => Some(DAY_MS),
            Self::LastThreeDays => Some(3 * DAY_MS),
            Self::LastMonth => Some(30 * DAY_MS),
            Self::Older => None,
        }
    }

    /// The group of a row last active `age_ms` ago.
    #[must_use]
    pub fn of_age(age_ms: i64) -> Self {
        Self::ALL
            .into_iter()
            .find(|group| group.upper_bound_ms().is_none_or(|bound| age_ms < bound))
            .unwrap_or(Self::Older)
    }
}

/// One non-empty age group with its rows in Forge order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecentThreadGroup<'a> {
    /// Which age the rows share.
    pub age: RecentThreadAge,
    /// The rows, newest activity first as the Forge ordered them.
    pub threads: Vec<&'a RecentThread>,
}

fn age_ms(thread: &RecentThread, now: UnixMillis) -> i64 {
    now.as_millis()
        .saturating_sub(thread.last_activity().as_millis())
}

/// Sorts the listing into its non-empty age groups at `now`.
#[must_use]
pub fn group_recent_threads(
    listing: &RecentThreadListing,
    now: UnixMillis,
) -> Vec<RecentThreadGroup<'_>> {
    RecentThreadAge::ALL
        .into_iter()
        .filter_map(|age| {
            let threads = listing
                .threads()
                .iter()
                .filter(|thread| RecentThreadAge::of_age(age_ms(thread, now)) == age)
                .collect::<Vec<_>>();
            (!threads.is_empty()).then_some(RecentThreadGroup { age, threads })
        })
        .collect()
}

/// The next instant after `now` at which any row moves to an older group;
/// `None` when every row is already in the oldest group.
#[must_use]
pub fn next_regrouping(listing: &RecentThreadListing, now: UnixMillis) -> Option<UnixMillis> {
    listing
        .threads()
        .iter()
        .filter_map(|thread| {
            RecentThreadAge::of_age(age_ms(thread, now))
                .upper_bound_ms()
                .map(|bound| thread.last_activity().as_millis().saturating_add(bound))
        })
        .filter(|at| *at > now.as_millis())
        .min()
        .map(UnixMillis::from_millis)
}
