//! The archived threads on the Threads page: each with when it was archived
//! and when the Forge erases it, the soonest first, and a way to restore it.

use std::collections::BTreeSet;

use artisan_domain::{ArchivedThreadListing, ThreadId, UnixMillis};
use gpui::Context;

use super::SettingsScreen;

/// What the archive section shows.
#[derive(Default)]
pub(super) struct ArchivedThreadsPage {
    /// The archived threads, once the Forge answered.
    pub(super) listing: Option<ArchivedThreadListing>,
    /// Threads being restored.
    pub(super) restoring: BTreeSet<ThreadId>,
    /// Why the archive could not be shown.
    pub(super) notice: Option<String>,
}

impl SettingsScreen {
    /// Shows the archived threads, the threads being restored, and any
    /// failure to read them.
    pub fn set_archived_threads(
        &mut self,
        listing: Option<ArchivedThreadListing>,
        restoring: BTreeSet<ThreadId>,
        notice: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.archived_threads = ArchivedThreadsPage {
            listing,
            restoring,
            notice,
        };
        cx.notify();
    }
}

/// How long from `from` until `to`, in the largest whole unit: "29 days",
/// "5 hours", "12 minutes"; under a minute reads as "1 minute".
pub(crate) fn span_until(from: UnixMillis, to: UnixMillis) -> String {
    const MINUTE: i64 = 60 * 1_000;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;
    let left = to.as_millis().saturating_sub(from.as_millis()).max(MINUTE);
    let (count, unit) = if left >= DAY {
        (left / DAY, "day")
    } else if left >= HOUR {
        (left / HOUR, "hour")
    } else {
        (left / MINUTE, "minute")
    };
    if count == 1 {
        format!("1 {unit}")
    } else {
        format!("{count} {unit}s")
    }
}

#[cfg(test)]
mod tests {
    use super::span_until;
    use artisan_domain::UnixMillis;

    #[test]
    fn spans_read_in_their_largest_whole_unit() {
        let at = |millis| UnixMillis::from_millis(millis);
        let minute = 60 * 1_000;
        let hour = 60 * minute;
        let day = 24 * hour;
        for (left, expected) in [
            (30 * day, "30 days"),
            (29 * day + 23 * hour, "29 days"),
            (day, "1 day"),
            (day - 1, "23 hours"),
            (hour, "1 hour"),
            (59 * minute, "59 minutes"),
            (10, "1 minute"),
            (-day, "1 minute"),
        ] {
            assert_eq!(span_until(at(0), at(left)), expected, "{left} ms");
        }
    }
}
