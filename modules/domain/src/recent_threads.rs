//! The Forge's cross-project listing of recently active threads.
//!
//! The Editor's sidebar renders this listing as it arrives: the Forge picks
//! the threads (saved threads only, newest activity first, bounded to one
//! page), orders them, and resolves each row's subtitle (the repository the
//! project publishes to, or the project's name). The Editor only groups the
//! rows by age for presentation.

use std::collections::HashSet;

use crate::model::{ThreadListingError, ThreadSummary};
use crate::text::DisplayName;
use crate::time::UnixMillis;

/// Maximum number of threads in one recent-threads listing.
pub const RECENT_THREADS_MAX: usize = 100;

/// Reads the recent threads across every attached project. The connection
/// that read them receives later changes pushed as
/// [`Event::RecentThreads`](crate::Event::RecentThreads).
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct ReadRecentThreads;

/// One recent thread with the Forge-resolved line shown beneath its title.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct RecentThread {
    /// The thread, naming its project.
    pub thread: ThreadSummary,
    /// The resolved repository (`owner/repo`, with the branch of a linked
    /// worktree) or the project's display name.
    pub subtitle: DisplayName,
}

impl RecentThread {
    /// When the thread was last active: its latest message, else its last
    /// update.
    #[must_use]
    pub fn last_activity(&self) -> UnixMillis {
        self.thread
            .last_message_at
            .unwrap_or(self.thread.updated_at)
    }
}

/// Recent threads across every attached project, newest activity first.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RecentThreadListing(Vec<RecentThread>);

impl RecentThreadListing {
    /// Builds a listing after enforcing its bound and thread uniqueness.
    ///
    /// # Errors
    ///
    /// Returns [`ThreadListingError::TooManyThreads`] beyond
    /// [`RECENT_THREADS_MAX`] rows and [`ThreadListingError::DuplicateThread`]
    /// naming the first repeated thread.
    pub fn new(threads: Vec<RecentThread>) -> Result<Self, ThreadListingError> {
        if threads.len() > RECENT_THREADS_MAX {
            return Err(ThreadListingError::TooManyThreads {
                count: threads.len(),
                maximum: RECENT_THREADS_MAX,
            });
        }
        let mut seen = HashSet::with_capacity(threads.len());
        for row in &threads {
            if !seen.insert(row.thread.thread_id.clone()) {
                return Err(ThreadListingError::DuplicateThread {
                    thread_id: row.thread.thread_id.clone(),
                });
            }
        }
        Ok(Self(threads))
    }

    /// The rows in Forge order.
    #[must_use]
    pub fn threads(&self) -> &[RecentThread] {
        &self.0
    }
}
