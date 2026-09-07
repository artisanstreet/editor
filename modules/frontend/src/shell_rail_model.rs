//! Pure thread-list grouping for the shell rail list.
//!
//! This is the dependency-free Rust counterpart of the rail-list derivations
//! in `modules/frontend/src/lib/root/thread-navigation.ts` as consumed by
//! `routes/components/thread-hover-rail.svelte`: [`PinnedThreads`],
//! [`SettledThreads`], and [`ThreadRailTimeGroups`]. It operates on the small
//! [`RailThread`] view values the host adapter already resolved (display
//! title, project label, live status, acknowledgement bit, and one resolved
//! activity millisecond stamp) rather than on protocol records, so timestamp
//! parsing and catalog reads stay outside this module.
//!
//! [`PinnedThreads`]: https://github.com/sandersonstabo/artisan-editor
//! [`SettledThreads`]: https://github.com/sandersonstabo/artisan-editor
//! [`ThreadRailTimeGroups`]: https://github.com/sandersonstabo/artisan-editor

#![forbid(unsafe_code)]

use crate::thread_navigation_core::{is_failed_status, thread_completed, thread_has_active_work};

/// One thread row as the rail list paints it.
///
/// `activity_ms` is the already-resolved `ThreadLastMessageAt` stamp (last
/// message, falling back to creation) as signed Unix epoch milliseconds.
/// `settled` is the `ThreadSettled` acknowledgement bit: reader-acknowledged
/// activity equals reader-visible activity. `awaiting_answer` is the exact
/// `ThreadIsAwaitingAnswer` status match.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RailThread<'a> {
    /// The durable thread identity used for active-row matching.
    pub thread_id: &'a str,
    /// The already-resolved display title.
    pub title: &'a str,
    /// The primary project's display name, when the thread has one.
    pub project_name: Option<&'a str>,
    /// The exact live status string.
    pub live_status: &'a str,
    /// Whether the reader has acknowledged the latest activity.
    pub settled: bool,
    /// Whether the thread reports the exact awaiting-answer status.
    pub awaiting_answer: bool,
    /// The resolved recency stamp, signed Unix epoch milliseconds.
    pub activity_ms: i64,
}

impl<'a> RailThread<'a> {
    /// Builds one rail row from already-resolved adapter values.
    #[must_use]
    pub const fn new(
        thread_id: &'a str,
        title: &'a str,
        project_name: Option<&'a str>,
        live_status: &'a str,
        settled: bool,
        awaiting_answer: bool,
        activity_ms: i64,
    ) -> Self {
        Self {
            thread_id,
            title,
            project_name,
            live_status,
            settled,
            awaiting_answer,
            activity_ms,
        }
    }

    /// Returns whether Forge still owns work on this thread.
    ///
    /// Mirrors `ThreadHasActiveWork`: every non-resting status except the two
    /// exact failure values.
    #[must_use]
    pub fn has_active_work(self) -> bool {
        thread_has_active_work(self.live_status)
    }

    /// Returns whether this thread reports an unread terminal outcome.
    ///
    /// Mirrors `ThreadNeedsAttention`: inactive, unacknowledged, and either
    /// completed or failed.
    #[must_use]
    pub fn needs_attention(self) -> bool {
        !self.has_active_work()
            && !self.settled
            && (thread_completed(self.live_status) || is_failed_status(self.live_status))
    }

    /// Returns whether this row belongs in the pinned working group.
    ///
    /// Mirrors `PinnedThreads`: active work plus unsettled terminal outcomes.
    /// A durable acknowledgement never outranks Forge's live run authority.
    #[must_use]
    pub fn is_pinned(self) -> bool {
        self.has_active_work() || self.needs_attention()
    }
}

/// The chronological settled-group identities, oldest last.
///
/// Labels are the exact `ThreadRailTimeGroups` strings. The `Today` group
/// carries no label in the legacy markup.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RailTimeGroupId {
    /// Threads active within the last day; rendered without a group label.
    Today,
    /// Threads active one up to two days ago.
    Yesterday,
    /// Threads active two up to three days ago.
    Last3Days,
    /// Threads active three up to seven days ago.
    Last7Days,
    /// Threads active seven or more days ago, including unparseable stamps.
    PastMonth,
}

impl RailTimeGroupId {
    /// Every group in the legacy partition order.
    pub const ALL: [Self; 5] = [
        Self::Today,
        Self::Yesterday,
        Self::Last3Days,
        Self::Last7Days,
        Self::PastMonth,
    ];

    /// Returns the exact legacy group label, if the group renders one.
    #[must_use]
    pub const fn label(self) -> Option<&'static str> {
        match self {
            Self::Today => None,
            Self::Yesterday => Some("Yesterday"),
            Self::Last3Days => Some("Last 3 days"),
            Self::Last7Days => Some("Last 7 days"),
            Self::PastMonth => Some("Past month"),
        }
    }

    /// Selects the group for one elapsed duration in milliseconds.
    ///
    /// Boundaries mirror the TypeScript partition exactly: under one day is
    /// today, then under two, three, and seven days, with everything else in
    /// the past month. A negative elapsed stamp (thread newer than `now_ms`)
    /// lands in today, matching the `<` comparison chain.
    #[must_use]
    pub const fn for_elapsed_ms(elapsed_ms: i64) -> Self {
        const DAY_MS: i64 = 24 * 60 * 60 * 1_000;
        if elapsed_ms < DAY_MS {
            Self::Today
        } else if elapsed_ms < 2 * DAY_MS {
            Self::Yesterday
        } else if elapsed_ms < 3 * DAY_MS {
            Self::Last3Days
        } else if elapsed_ms < 7 * DAY_MS {
            Self::Last7Days
        } else {
            Self::PastMonth
        }
    }
}

/// One non-empty settled time group in partition order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RailTimeGroup<'a> {
    /// The group's chronological identity.
    pub id: RailTimeGroupId,
    /// The group's rows, newest first.
    pub threads: Vec<&'a RailThread<'a>>,
}

impl RailTimeGroup<'_> {
    /// Returns the exact legacy group label, if the group renders one.
    #[must_use]
    pub const fn label(&self) -> Option<&'static str> {
        self.id.label()
    }
}

/// The rail list split into its pinned card and settled time groups.
///
/// Both collections hold borrowed rows newest-first; the sort is stable, so
/// equal activity stamps keep their input order, matching the legacy
/// inbox sort.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RailListModel<'a> {
    /// Active and attention-needing rows, newest first.
    pub pinned: Vec<&'a RailThread<'a>>,
    /// Non-empty settled groups in chronological partition order.
    pub groups: Vec<RailTimeGroup<'a>>,
}

impl RailListModel<'_> {
    /// Returns whether the list carries any row at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pinned.is_empty() && self.groups.is_empty()
    }

    /// Returns the number of rows across the pinned card and all groups.
    #[must_use]
    pub fn len(&self) -> usize {
        self.pinned.len()
            + self
                .groups
                .iter()
                .map(|group| group.threads.len())
                .sum::<usize>()
    }
}

/// One settled bucket per [`RailTimeGroupId`], in [`RailTimeGroupId::ALL`]
/// order: today, yesterday, last three days, last seven days, past month.
type SettledBuckets<'a> = (
    Vec<&'a RailThread<'a>>,
    Vec<&'a RailThread<'a>>,
    Vec<&'a RailThread<'a>>,
    Vec<&'a RailThread<'a>>,
    Vec<&'a RailThread<'a>>,
);
/// Splits one thread snapshot into the pinned card and settled time groups.
///
/// Mirrors the Svelte derivations: `PinnedThreads` feeds the working card and
/// `ThreadRailTimeGroups(SettledThreads(threads), now_ms)` feeds the history
/// list. Empty groups are omitted, exactly like the legacy `.filter` on the
/// partition result.
#[must_use]
pub fn rail_list_model<'threads>(
    threads: &'threads [RailThread<'threads>],
    now_ms: i64,
) -> RailListModel<'threads> {
    let mut ordered: Vec<&RailThread> = threads.iter().collect();
    // Newest activity first; stable so equal stamps keep catalog order.
    ordered.sort_by_key(|thread| std::cmp::Reverse(thread.activity_ms));

    let mut pinned = Vec::new();
    let mut settled: Vec<&RailThread> = Vec::new();
    for thread in ordered {
        if thread.is_pinned() {
            pinned.push(thread);
        } else {
            settled.push(thread);
        }
    }

    let (mut today, mut yesterday, mut last_3_days, mut last_7_days, mut past_month): SettledBuckets<'_> =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for thread in settled {
        let elapsed_ms = now_ms.saturating_sub(thread.activity_ms);
        match RailTimeGroupId::for_elapsed_ms(elapsed_ms) {
            RailTimeGroupId::Today => today.push(thread),
            RailTimeGroupId::Yesterday => yesterday.push(thread),
            RailTimeGroupId::Last3Days => last_3_days.push(thread),
            RailTimeGroupId::Last7Days => last_7_days.push(thread),
            RailTimeGroupId::PastMonth => past_month.push(thread),
        }
    }

    let mut groups = Vec::with_capacity(RailTimeGroupId::ALL.len());
    for (id, threads) in [
        (RailTimeGroupId::Today, today),
        (RailTimeGroupId::Yesterday, yesterday),
        (RailTimeGroupId::Last3Days, last_3_days),
        (RailTimeGroupId::Last7Days, last_7_days),
        (RailTimeGroupId::PastMonth, past_month),
    ] {
        if !threads.is_empty() {
            groups.push(RailTimeGroup { id, threads });
        }
    }

    RailListModel { pinned, groups }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn row<'a>(
        id: &'a str,
        status: &'a str,
        settled: bool,
        activity_ms: i64,
    ) -> RailThread<'a> {
        RailThread::new(id, id, None, status, settled, false, activity_ms)
    }

    #[test]
    fn pinned_holds_active_work_and_unread_outcomes() {
        let threads = [
            row("running", "Running", true, 100),
            row("unread-failed", "Failed to complete", false, 90),
            row("read-done", "Complete", true, 80),
        ];
        let model = rail_list_model(&threads, 1_000);
        let pinned: Vec<&str> = model.pinned.iter().map(|thread| thread.thread_id).collect();
        assert_eq!(pinned, vec!["running", "unread-failed"]);
        assert_eq!(model.groups.len(), 1);
        assert_eq!(model.groups[0].id, RailTimeGroupId::Today);
        assert_eq!(model.groups[0].threads[0].thread_id, "read-done");
    }

    #[test]
    fn settled_thread_leaves_the_pinned_group() {
        let threads = [
            row("done", "Complete", false, 100),
            row("acked", "Complete", true, 90),
        ];
        let model = rail_list_model(&threads, 1_000);
        assert_eq!(model.pinned.len(), 1);
        assert_eq!(model.pinned[0].thread_id, "done");
    }

    #[test]
    fn time_group_boundaries_match_legacy_partition() {
        const HOUR_MS: i64 = 3_600_000;
        const DAY_MS: i64 = 24 * HOUR_MS;
        let now_ms = 10 * DAY_MS;
        let threads = [
            row("today", "Idle", true, now_ms - HOUR_MS),
            row("yesterday", "Idle", true, now_ms - DAY_MS),
            row("three", "Idle", true, now_ms - 2 * DAY_MS),
            row("seven", "Idle", true, now_ms - 3 * DAY_MS),
            row("month", "Idle", true, now_ms - 7 * DAY_MS),
            row("old", "Idle", true, now_ms - 60 * DAY_MS),
        ];
        let model = rail_list_model(&threads, now_ms);
        let ids: Vec<RailTimeGroupId> = model.groups.iter().map(|group| group.id).collect();
        assert_eq!(
            ids,
            vec![
                RailTimeGroupId::Today,
                RailTimeGroupId::Yesterday,
                RailTimeGroupId::Last3Days,
                RailTimeGroupId::Last7Days,
                RailTimeGroupId::PastMonth,
            ]
        );
        assert_eq!(model.groups[4].threads.len(), 2);
    }

    #[test]
    fn group_labels_match_legacy_strings() {
        assert_eq!(RailTimeGroupId::Today.label(), None);
        assert_eq!(RailTimeGroupId::Yesterday.label(), Some("Yesterday"));
        assert_eq!(RailTimeGroupId::Last3Days.label(), Some("Last 3 days"));
        assert_eq!(RailTimeGroupId::Last7Days.label(), Some("Last 7 days"));
        assert_eq!(RailTimeGroupId::PastMonth.label(), Some("Past month"));
    }

    #[test]
    fn empty_snapshot_yields_empty_model() {
        let model = rail_list_model(&[], 0);
        assert!(model.is_empty());
        assert_eq!(model.len(), 0);
    }
}
