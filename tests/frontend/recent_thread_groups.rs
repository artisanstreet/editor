//! Age grouping of the sidebar's recent threads: boundaries at exactly 24
//! hours, 3 days and 30 days, empty groups hidden, Forge order kept within a
//! group, and regrouping as time passes.

use artisan_domain::{
    DisplayName, ProjectId, RecentThread, RecentThreadListing, ThreadId, ThreadSummary,
    ThreadTitle, UnixMillis,
};
use artisan_frontend::recent_thread_groups::{
    RecentThreadAge, group_recent_threads, next_regrouping,
};

const HOUR: i64 = 60 * 60 * 1_000;
const DAY: i64 = 24 * HOUR;
const NOW: i64 = 1_000 * DAY;

fn row(id: &str, activity: i64) -> RecentThread {
    RecentThread {
        thread: ThreadSummary {
            has_started_response: true,
            has_active_work: false,
            last_message_at: Some(UnixMillis::from_millis(activity)),
            thread_id: ThreadId::parse(id).expect("thread id"),
            project_id: ProjectId::parse("project").expect("project id"),
            title: ThreadTitle::parse(id).expect("title"),
            created_at: UnixMillis::from_millis(0),
            updated_at: UnixMillis::from_millis(0),
        },
        subtitle: DisplayName::parse("owner/repo").expect("subtitle"),
    }
}

fn listing(rows: Vec<RecentThread>) -> RecentThreadListing {
    RecentThreadListing::new(rows).expect("listing")
}

fn grouped(listing: &RecentThreadListing, now: i64) -> Vec<(RecentThreadAge, Vec<&str>)> {
    group_recent_threads(listing, UnixMillis::from_millis(now))
        .into_iter()
        .map(|group| {
            (
                group.age,
                group
                    .threads
                    .iter()
                    .map(|thread| thread.thread.thread_id.as_str())
                    .collect(),
            )
        })
        .collect()
}

#[test]
fn boundaries_belong_to_the_older_group() {
    let cases = [
        (0, RecentThreadAge::LastDay),
        (DAY - 1, RecentThreadAge::LastDay),
        (DAY, RecentThreadAge::LastThreeDays),
        (3 * DAY - 1, RecentThreadAge::LastThreeDays),
        (3 * DAY, RecentThreadAge::LastMonth),
        (30 * DAY - 1, RecentThreadAge::LastMonth),
        (30 * DAY, RecentThreadAge::Older),
        (400 * DAY, RecentThreadAge::Older),
        // A clock behind the Forge's still shows the thread as recent.
        (-HOUR, RecentThreadAge::LastDay),
    ];
    for (age, expected) in cases {
        assert_eq!(RecentThreadAge::of_age(age), expected, "age {age}");
    }
}

#[test]
fn groups_are_labelled_youngest_first_and_empty_ones_are_hidden() {
    let rows = listing(vec![
        row("minutes", NOW - 5 * 60 * 1_000),
        row("hours", NOW - 23 * HOUR),
        row("exactly-a-day", NOW - DAY),
        row("exactly-thirty-days", NOW - 30 * DAY),
        row("years", NOW - 700 * DAY),
    ]);
    assert_eq!(
        grouped(&rows, NOW),
        [
            (RecentThreadAge::LastDay, vec!["minutes", "hours"]),
            (RecentThreadAge::LastThreeDays, vec!["exactly-a-day"]),
            (RecentThreadAge::Older, vec!["exactly-thirty-days", "years"]),
        ]
    );
    assert_eq!(
        RecentThreadAge::ALL.map(RecentThreadAge::label),
        ["Last 24 hours", "Last 3 days", "Last 30 days", "Older"]
    );
    assert!(grouped(&listing(Vec::new()), NOW).is_empty());
}

#[test]
fn rows_keep_the_forge_order_within_a_group() {
    let rows = listing(vec![
        row("newest", NOW - HOUR),
        row("middle", NOW - 2 * HOUR),
        row("oldest", NOW - 3 * HOUR),
    ]);
    assert_eq!(
        grouped(&rows, NOW),
        [(RecentThreadAge::LastDay, vec!["newest", "middle", "oldest"])]
    );
}

#[test]
fn rows_move_to_older_groups_as_time_passes() {
    let rows = listing(vec![
        row("recent", NOW - 2 * HOUR),
        row("days", NOW - 2 * DAY),
    ]);
    let next = next_regrouping(&rows, UnixMillis::from_millis(NOW)).expect("a boundary ahead");
    assert_eq!(next, UnixMillis::from_millis(NOW - 2 * HOUR + DAY));
    assert_eq!(
        grouped(&rows, next.as_millis() - 1),
        [
            (RecentThreadAge::LastDay, vec!["recent"]),
            (RecentThreadAge::LastThreeDays, vec!["days"]),
        ]
    );
    assert_eq!(
        grouped(&rows, next.as_millis()),
        [(RecentThreadAge::LastThreeDays, vec!["recent", "days"])]
    );
    assert_eq!(
        next_regrouping(&rows, next),
        Some(UnixMillis::from_millis(NOW - 2 * DAY + 3 * DAY))
    );
    let settled = listing(vec![row("ancient", NOW - 90 * DAY)]);
    assert_eq!(
        next_regrouping(&settled, UnixMillis::from_millis(NOW)),
        None
    );
}
