//! The reader's changes to where a thread lives and what it is called, and
//! the archive they can restore from. Each answer is the archived listing,
//! so the Settings archive stays current; the sidebar's listing follows as
//! a pushed change.

use artisan_database::{Repository, RepositoryError};
use artisan_domain::{
    ARCHIVED_THREADS_MAX, ArchivedThread, ArchivedThreadListing, RequestId, ThreadPlacement,
    UpdateThread,
};
use artisan_protocol::{ProtocolFailure, ResponsePayload, ServerResponse};

use crate::conversation_commit_notifier::ConversationCommitNotifier;
use crate::project_subtitles::ProjectSubtitles;

use super::failures::{outcome, repository_failure};
use super::recent_threads::present_threads;
use super::{RequestHandler, origin_clock_failure};

impl RequestHandler {
    /// Answers the archived threads.
    pub(super) async fn archived_threads_outcome(
        &self,
        request_id: &RequestId,
    ) -> Result<ServerResponse, ProtocolFailure> {
        let listing = self
            .archived_threads()
            .await
            .map_err(|error| repository_failure(&error, request_id))?;
        Ok(outcome(
            request_id,
            ResponsePayload::ArchivedThreads(listing),
        ))
    }

    /// Applies one change to a thread, wakes the connections that show it,
    /// and answers the archived threads.
    pub(super) async fn update_thread_outcome(
        &self,
        request_id: &RequestId,
        update: &UpdateThread,
    ) -> Result<ServerResponse, ProtocolFailure> {
        let at = self
            .origin
            .acceptance_instant()
            .map_err(|error| origin_clock_failure(error, request_id))?;
        self.repository
            .update_thread(&update.thread_id, &update.change, at)
            .await
            .map_err(|error| repository_failure(&error, request_id))?;
        if let Some(notifier) = &self.conversation_commit_notifier {
            let _ = notifier.publish(&update.thread_id);
        }
        self.archived_threads_outcome(request_id).await
    }

    async fn archived_threads(&self) -> Result<ArchivedThreadListing, RepositoryError> {
        read_archived_threads(
            &self.repository,
            self.project_subtitles.as_ref(),
            self.conversation_commit_notifier.as_ref(),
        )
        .await
    }
}

/// Reads the archived threads, presented as the sidebar showed them.
async fn read_archived_threads(
    repository: &Repository,
    subtitles: Option<&ProjectSubtitles>,
    notifier: Option<&ConversationCommitNotifier>,
) -> Result<ArchivedThreadListing, RepositoryError> {
    let archived = repository
        .list_archived_threads(ARCHIVED_THREADS_MAX)
        .await?;
    let (threads, moments): (Vec<_>, Vec<_>) = archived
        .into_iter()
        .map(|(thread, at)| ((thread, ThreadPlacement::History), at))
        .unzip();
    let ids = threads
        .iter()
        .map(|(thread, _)| thread.thread_id.clone())
        .collect::<Vec<_>>();
    let presented = present_threads(repository, subtitles, notifier, threads).await?;
    let rows = presented
        .into_iter()
        .filter_map(|thread| {
            let index = ids.iter().position(|id| *id == thread.thread.thread_id)?;
            Some(ArchivedThread {
                archived_at: moments[index],
                thread,
            })
        })
        .collect();
    ArchivedThreadListing::new(rows).map_err(|source| RepositoryError::ThreadListing { source })
}
