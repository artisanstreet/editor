//! Where a thread lives in the Editor: the sidebar, its History, or the
//! archive.
//!
//! A thread stays in the sidebar until the reader settles it into History,
//! or until it has gone a day without activity or viewing, when the Forge
//! files it there. New activity brings a settled thread back; viewing does
//! not, so opening a History row never moves it. Archiving takes a thread
//! out of every listing, and the Forge erases it for good
//! [`ARCHIVE_RETENTION_MS`] after it was archived unless it is restored.

use std::collections::HashSet;

use crate::identifiers::{RequestId, ThreadId};
use crate::model::ThreadListingError;
use crate::recent_threads::RecentThread;
use crate::text::ThreadTitle;
use crate::time::UnixMillis;

const DAY_MS: i64 = 24 * 60 * 60 * 1_000;

/// How long a thread may sit without activity or viewing before the Forge
/// files it into History.
pub const HISTORY_IDLE_MS: i64 = DAY_MS;

/// How long an archived thread is kept before the Forge erases it.
pub const ARCHIVE_RETENTION_MS: i64 = 30 * DAY_MS;

/// Maximum number of threads in one archived-threads listing.
pub const ARCHIVED_THREADS_MAX: usize = 500;

/// Where a listed thread sits in the sidebar.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum ThreadPlacement {
    /// In view at the top of the sidebar.
    #[default]
    Inbox,
    /// Settled into History, by the reader or after a quiet day.
    History,
}

/// One change the reader makes to a thread.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ThreadChange {
    /// Gives the thread the reader's own title, which generated titles never
    /// replace.
    Rename(ThreadTitle),
    /// Settles the thread into History until it next has activity.
    Settle,
    /// Moves the thread to the archive, from which the Forge erases it after
    /// [`ARCHIVE_RETENTION_MS`].
    Archive,
    /// Restores an archived thread to the sidebar.
    Unarchive,
}

/// Applies one [`ThreadChange`], answered with the archived threads.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct UpdateThread {
    /// Client-minted request identity.
    pub request_id: RequestId,
    /// The thread to change.
    pub thread_id: ThreadId,
    /// What to change.
    pub change: ThreadChange,
}

/// Reads the archived threads, the soonest to be erased first.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct ReadArchivedThreads;

/// One archived thread and when it was archived.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ArchivedThread {
    /// The thread as the sidebar showed it.
    pub thread: RecentThread,
    /// When the reader archived it.
    pub archived_at: UnixMillis,
}

impl ArchivedThread {
    /// When the Forge erases the thread.
    #[must_use]
    pub const fn erases_at(&self) -> UnixMillis {
        UnixMillis::from_millis(
            self.archived_at
                .as_millis()
                .saturating_add(ARCHIVE_RETENTION_MS),
        )
    }
}

/// The archived threads, the soonest to be erased first.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ArchivedThreadListing(Vec<ArchivedThread>);

impl ArchivedThreadListing {
    /// Builds a listing after enforcing its bound and thread uniqueness,
    /// ordering the rows by when they are erased.
    ///
    /// # Errors
    ///
    /// Returns [`ThreadListingError::TooManyThreads`] beyond
    /// [`ARCHIVED_THREADS_MAX`] rows and
    /// [`ThreadListingError::DuplicateThread`] naming the first repeated
    /// thread.
    pub fn new(mut threads: Vec<ArchivedThread>) -> Result<Self, ThreadListingError> {
        if threads.len() > ARCHIVED_THREADS_MAX {
            return Err(ThreadListingError::TooManyThreads {
                count: threads.len(),
                maximum: ARCHIVED_THREADS_MAX,
            });
        }
        let mut seen = HashSet::with_capacity(threads.len());
        for row in &threads {
            if !seen.insert(row.thread.thread.thread_id.clone()) {
                return Err(ThreadListingError::DuplicateThread {
                    thread_id: row.thread.thread.thread_id.clone(),
                });
            }
        }
        threads.sort_by(|left, right| {
            left.archived_at.cmp(&right.archived_at).then_with(|| {
                left.thread
                    .thread
                    .thread_id
                    .cmp(&right.thread.thread.thread_id)
            })
        });
        Ok(Self(threads))
    }

    /// The rows, the soonest to be erased first.
    #[must_use]
    pub fn threads(&self) -> &[ArchivedThread] {
        &self.0
    }
}
