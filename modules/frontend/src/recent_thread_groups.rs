//! Sections and age groups of the sidebar's recent threads.
//!
//! The Forge decides which threads are recent, orders them newest activity
//! first, and resolves each row's subtitle; the Editor only sorts the rows
//! it received for presentation, as a pure function of the listing and the
//! clock.
//!
//! The sidebar is an inbox ([`SidebarSection::Inbox`]): a thread stays at
//! the top until the reader settles it, or the Forge files it after a day
//! without activity or viewing, into `History`, which keeps its age groups
//! inside. Reading a thread never moves it. Live work, an open question, or
//! an unseen outcome keeps a settled thread at the top until it is idle and
//! read again. Groups keep the Forge's order, and empty groups are not
//! shown. As time passes a row moves to an older age group; the next
//! instant any row crosses a boundary is [`next_regrouping`].

#![forbid(unsafe_code)]

use artisan_domain::{
    RecentThread, RecentThreadListing, ThreadAttention, ThreadPlacement, UnixMillis,
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
    /// In view: not settled yet, or something live or unseen in a settled
    /// thread (a run the Forge owns, an open question or approval, or an
    /// outcome that finished or failed since the reader last read it).
    /// Always shown.
    Inbox,
    /// Settled, idle, and read.
    History,
}

impl SidebarSection {
    /// The section of one thread.
    ///
    /// A thread only reaches History by being settled; reading or leaving
    /// it never moves it, so a row never jumps away from the pointer.
    #[must_use]
    pub fn of(row: &RecentThread) -> Self {
        let thread = &row.thread;
        let needs_reader = thread.has_active_work || thread.attention != ThreadAttention::None;
        match row.placement {
            ThreadPlacement::History if !needs_reader => Self::History,
            ThreadPlacement::Inbox | ThreadPlacement::History => Self::Inbox,
        }
    }
}

/// The sidebar's recent threads sorted into sections at one instant.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SidebarSections<'a> {
    /// Rows in view, newest activity first.
    pub inbox: Vec<&'a RecentThread>,
    /// Settled rows in their non-empty age groups.
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
        match SidebarSection::of(thread) {
            SidebarSection::Inbox => sections.inbox.push(thread),
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
    use artisan_domain::{DisplayName, ProjectId, ThreadId, ThreadSummary, ThreadTitle};

    use super::*;

    fn settled(working: bool, attention: ThreadAttention) -> RecentThread {
        RecentThread {
            thread: ThreadSummary {
                has_started_response: true,
                has_active_work: working,
                attention,
                last_message_at: None,
                thread_id: ThreadId::parse("thread").expect("thread id"),
                project_id: ProjectId::parse("project").expect("project id"),
                title: ThreadTitle::parse("Thread").expect("title"),
                created_at: UnixMillis::from_millis(1),
                updated_at: UnixMillis::from_millis(1),
            },
            subtitle: DisplayName::parse("project").expect("subtitle"),
            project_icon: Default::default(),
            placement: ThreadPlacement::History,
        }
    }

    #[test]
    fn live_and_unseen_settled_threads_stay_out_of_history() {
        let of = |working, attention| SidebarSection::of(&settled(working, attention));
        assert_eq!(of(false, ThreadAttention::Finished), SidebarSection::Inbox);
        assert_eq!(of(false, ThreadAttention::Failed), SidebarSection::Inbox);
        assert_eq!(
            of(true, ThreadAttention::AwaitingAnswer),
            SidebarSection::Inbox
        );
        // Live work stays in the main list.
        assert_eq!(of(true, ThreadAttention::None), SidebarSection::Inbox);
        assert_eq!(of(true, ThreadAttention::Finished), SidebarSection::Inbox);
        assert_eq!(of(false, ThreadAttention::None), SidebarSection::History);
    }
}
