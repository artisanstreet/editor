//! The recent threads across every attached project, as the Editor's
//! sidebar shows them: the Forge picks the rows (saved threads, newest
//! activity first, one bounded page), marks live work, and resolves each
//! row's subtitle. A connection reads them once; its delivery driver then
//! pushes their changes.

use artisan_database::{Repository, RepositoryError};
use artisan_domain::{
    DisplayName, RECENT_THREADS_MAX, RecentThread, RecentThreadListing, RequestId, ThreadSummary,
};
use artisan_protocol::{ProtocolFailure, ResponsePayload, ServerResponse};

use crate::conversation_commit_notifier::ConversationCommitNotifier;
use crate::project_subtitles::ProjectSubtitles;
use crate::run_cancellation::RunCancellationRegistry;

use super::failures::{outcome, repository_failure};
use super::queries::run_live_status;
use super::{ConversationConnectionContext, RequestHandler};

impl RequestHandler {
    /// Answers a recent-threads read.
    pub(super) async fn recent_threads_outcome(
        &self,
        request_id: &RequestId,
    ) -> Result<ServerResponse, ProtocolFailure> {
        let listing = read_recent_threads(
            &self.repository,
            self.run_cancellation.as_ref(),
            self.project_subtitles.as_ref(),
            self.conversation_commit_notifier.as_ref(),
        )
        .await
        .map_err(|error| repository_failure(&error, request_id))?;
        Ok(outcome(request_id, ResponsePayload::RecentThreads(listing)))
    }
}

impl ConversationConnectionContext {
    /// Reads the recent threads this connection shows.
    pub(crate) async fn recent_threads(&self) -> Result<RecentThreadListing, RepositoryError> {
        read_recent_threads(
            &self.repository,
            self.run_cancellation.as_ref(),
            self.project_subtitles.as_ref(),
            Some(&self.notifier),
        )
        .await
    }

    /// Advances whenever a project's resolved subtitle may have changed.
    pub(crate) fn subtitle_generation(&self) -> u64 {
        self.project_subtitles
            .as_ref()
            .map_or(0, ProjectSubtitles::generation)
    }
}

/// Reads the current recent-threads listing.
///
/// A thread has live work when the run registry holds a run whose durable
/// lifecycle is not settled, exactly as the project thread listing marks
/// it. Subtitles come from the cached repository observations; without the
/// subtitle cache every row shows its project's display name.
async fn read_recent_threads(
    repository: &Repository,
    run_cancellation: Option<&RunCancellationRegistry>,
    subtitles: Option<&ProjectSubtitles>,
    notifier: Option<&ConversationCommitNotifier>,
) -> Result<RecentThreadListing, RepositoryError> {
    let projects = repository.list_projects().await?;
    let mut threads = repository.list_recent_threads(RECENT_THREADS_MAX).await?;
    mark_live_work(repository, run_cancellation, &mut threads).await?;
    let resolved = subtitles.map(|subtitles| {
        let unobserved = ConversationCommitNotifier::new();
        subtitles.subtitles(projects.projects(), notifier.unwrap_or(&unobserved))
    });
    let subtitle = |thread: &ThreadSummary| -> Option<DisplayName> {
        resolved
            .as_ref()
            .and_then(|resolved| resolved.get(&thread.project_id).cloned())
            .or_else(|| {
                projects
                    .projects()
                    .iter()
                    .find(|project| project.project_id == thread.project_id)
                    .map(|project| project.display_name.clone())
            })
    };
    let rows = threads
        .into_iter()
        .filter_map(|thread| {
            Some(RecentThread {
                subtitle: subtitle(&thread)?,
                thread,
            })
        })
        .collect();
    RecentThreadListing::new(rows).map_err(|source| RepositoryError::ThreadListing { source })
}

async fn mark_live_work(
    repository: &Repository,
    run_cancellation: Option<&RunCancellationRegistry>,
    threads: &mut [ThreadSummary],
) -> Result<(), RepositoryError> {
    let Some(registry) = run_cancellation else {
        return Ok(());
    };
    for thread in threads.iter_mut() {
        // An unavailable registry reads as no live run: the listing is
        // presentation, never a reason to fail the connection.
        let Some(run) = registry.active_run(&thread.thread_id).ok().flatten() else {
            continue;
        };
        thread.has_active_work = repository
            .read_assistant_run_status(&thread.thread_id, &run)
            .await?
            .is_some_and(|(lifecycle, _)| run_live_status(&lifecycle).is_some());
    }
    Ok(())
}
