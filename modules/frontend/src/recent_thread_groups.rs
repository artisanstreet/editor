//! Sections and age groups of the sidebar's recent threads.
//!
//! The Forge decides which threads are recent, orders them newest activity
//! first, and resolves each row's subtitle; the Editor only sorts the rows
//! it received for presentation, as a pure function of the listing and the
//! clock.
//!
//! The sidebar is an inbox: only threads with something the reader has not
//! seen show at the top ([`SidebarSection::Unread`]). Threads the Forge is
//! still running collapse into one `Working` group, and everything read
//! and idle collapses into `History`, which keeps its age groups inside.
//! Groups keep the Forge's order, and empty groups are not shown. As time
//! passes a row moves to an older age group; the next instant any row
//! crosses a boundary is [`next_regrouping`].

#![forbid(unsafe_code)]

use artisan_domain::{
    RecentThread, RecentThreadListing, ThreadAttention, ThreadSummary, UnixMillis,
};

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
    group_by_age(&listing.threads().iter().collect::<Vec<_>>(), now)
}

/// Sorts rows, kept in their given order, into non-empty age groups.
fn group_by_age<'a>(threads: &[&'a RecentThread], now: UnixMillis) -> Vec<RecentThreadGroup<'a>> {
    RecentThreadAge::ALL
        .into_iter()
        .filter_map(|age| {
            let threads = threads
                .iter()
                .copied()
                .filter(|thread| RecentThreadAge::of_age(age_ms(thread, now)) == age)
                .collect::<Vec<_>>();
            (!threads.is_empty()).then_some(RecentThreadGroup { age, threads })
        })
        .collect()
}

/// Which sidebar section a thread belongs to.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SidebarSection {
    /// Something the reader has not seen: an open question or approval, or
    /// an outcome that finished or failed since they last read the thread.
    /// Always shown.
    Unread,
    /// The Forge owns a live run and nothing waits on the reader.
    Working,
    /// Read and idle.
    History,
}

impl SidebarSection {
    /// The section of one thread.
    ///
    /// Mirrors the row's state dot: waiting on the reader outranks the work
    /// it blocks, and a new run after an unread outcome reads as working.
    /// An unread outcome stays unread while its thread is open, so opening
    /// it never pulls the row out from under the pointer; leaving the
    /// thread marks it read and moves it to history.
    #[must_use]
    pub fn of(thread: &ThreadSummary) -> Self {
        match thread.attention {
            ThreadAttention::AwaitingAnswer => Self::Unread,
            _ if thread.has_active_work => Self::Working,
            ThreadAttention::Finished | ThreadAttention::Failed => Self::Unread,
            ThreadAttention::None => Self::History,
        }
    }
}

/// The sidebar's recent threads sorted into sections at one instant.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SidebarSections<'a> {
    /// Unread rows, newest activity first.
    pub unread: Vec<&'a RecentThread>,
    /// Working rows, newest activity first.
    pub working: Vec<&'a RecentThread>,
    /// Read and idle rows in their non-empty age groups.
    pub history: Vec<RecentThreadGroup<'a>>,
}

impl SidebarSections<'_> {
    /// How many rows the history holds across its age groups.
    #[must_use]
    pub fn history_len(&self) -> usize {
        self.history.iter().map(|group| group.threads.len()).sum()
    }
}

/// Sorts the listing into the sidebar's sections at `now`.
#[must_use]
pub fn section_recent_threads(
    listing: &RecentThreadListing,
    now: UnixMillis,
) -> SidebarSections<'_> {
    let mut sections = SidebarSections::default();
    let mut history = Vec::new();
    for thread in listing.threads() {
        match SidebarSection::of(&thread.thread) {
            SidebarSection::Unread => sections.unread.push(thread),
            SidebarSection::Working => sections.working.push(thread),
            SidebarSection::History => history.push(thread),
        }
    }
    sections.history = group_by_age(&history, now);
    sections
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

#[cfg(test)]
mod tests {
    use artisan_domain::{ProjectId, ThreadId, ThreadTitle};

    use super::*;

    fn thread(working: bool, attention: ThreadAttention) -> ThreadSummary {
        ThreadSummary {
            has_started_response: true,
            has_active_work: working,
            attention,
            last_message_at: None,
            thread_id: ThreadId::parse("thread").expect("thread id"),
            project_id: ProjectId::parse("project").expect("project id"),
            title: ThreadTitle::parse("Thread").expect("title"),
            created_at: UnixMillis::from_millis(1),
            updated_at: UnixMillis::from_millis(1),
        }
    }

    #[test]
    fn only_unseen_threads_stay_out_of_the_collapsed_groups() {
        let of = |working, attention| SidebarSection::of(&thread(working, attention));
        assert_eq!(of(false, ThreadAttention::Finished), SidebarSection::Unread);
        assert_eq!(of(false, ThreadAttention::Failed), SidebarSection::Unread);
        // The reader's turn outranks the work it blocks.
        assert_eq!(
            of(true, ThreadAttention::AwaitingAnswer),
            SidebarSection::Unread
        );
        assert_eq!(of(true, ThreadAttention::None), SidebarSection::Working);
        // A new run after an unread outcome reads as working.
        assert_eq!(of(true, ThreadAttention::Finished), SidebarSection::Working);
        assert_eq!(of(false, ThreadAttention::None), SidebarSection::History);
    }
}
