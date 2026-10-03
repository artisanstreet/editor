//! Process-wide subtitles of attached projects for the recent-threads
//! listing.
//!
//! A subtitle derives purely from the latest repository observation of the
//! project's root ([`crate::project_subtitle_policy`]). Observations are
//! cached per root so producing a listing never waits on Git: the listing
//! uses the display name (or the previous observation) until a background
//! refresh observes the root. [`ProjectSubtitles::keep_current`] observes
//! every attached root from startup on, reads a stale root again after
//! [`OBSERVATION_FRESHNESS`], and retries a failed read with backoff from
//! [`FAILED_OBSERVATION_RETRY`]; a listing that finds a root due starts the
//! same refresh at once. A refresh that changed an observation advances
//! [`ProjectSubtitles::generation`] and wakes every connection, whose
//! delivery then pushes the changed listing.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use artisan_database::Repository;
use artisan_domain::{DisplayName, ProjectId, ProjectSummary, RecentProjectIcon, RootPath};
use artisan_transport::CancelHandle;

use crate::conversation_commit_notifier::ConversationCommitNotifier;
use crate::project_repository_service::{
    PROJECT_REPOSITORY_QUERY_TIMEOUT, ProjectRepositoryService, RepositoryObservation,
};
use crate::project_subtitle_policy::project_subtitle;

/// How long one observation of a root stands before it is read again.
pub const OBSERVATION_FRESHNESS: Duration = Duration::from_secs(60);

/// How soon a root whose read failed is read again; the wait doubles with
/// each further failure, up to [`OBSERVATION_FRESHNESS`].
pub const FAILED_OBSERVATION_RETRY: Duration = Duration::from_secs(2);

/// Cached repository observations per project root, shared by every
/// connection. Cloning shares the cache.
#[derive(Clone)]
pub struct ProjectSubtitles {
    shared: Arc<Shared>,
}

struct Shared {
    service: ProjectRepositoryService,
    state: Mutex<CacheState>,
    generation: AtomicU64,
}

#[derive(Default)]
struct CacheState {
    observed: HashMap<RootPath, Observed>,
    refreshing: bool,
}

struct Observed {
    /// `None` when the root could not be read (its state is unknown).
    observation: Option<RepositoryObservation>,
    at: Instant,
    /// Consecutive failed reads, zero after a successful one.
    failures: u32,
    icon: RecentProjectIcon,
    icon_identity: Option<String>,
    icon_at: Option<Instant>,
    icon_pending: bool,
}

impl Observed {
    /// When this root is next read.
    fn due(&self) -> Instant {
        let wait = if self.failures == 0 {
            OBSERVATION_FRESHNESS
        } else {
            FAILED_OBSERVATION_RETRY
                .saturating_mul(1 << self.failures.saturating_sub(1).min(5))
                .min(OBSERVATION_FRESHNESS)
        };
        self.at + wait
    }
}

/// Releases the single refresh slot however the refresh ends.
struct RefreshSlot(ProjectSubtitles);

impl Drop for RefreshSlot {
    fn drop(&mut self) {
        self.0.lock().refreshing = false;
    }
}

impl std::fmt::Debug for ProjectSubtitles {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ProjectSubtitles { <payload-free> }")
    }
}

impl ProjectSubtitles {
    /// Creates an empty cache observing roots through `service`.
    #[must_use]
    pub fn new(service: ProjectRepositoryService) -> Self {
        Self {
            shared: Arc::new(Shared {
                service,
                state: Mutex::new(CacheState::default()),
                generation: AtomicU64::new(0),
            }),
        }
    }

    /// Advances whenever a refresh changed an observation.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.shared.generation.load(Ordering::Acquire)
    }

    /// Reads only cached artwork; never waits on files, Git, or a network.
    #[must_use]
    pub fn icons(&self, projects: &[ProjectSummary]) -> HashMap<ProjectId, RecentProjectIcon> {
        let state = self.lock();
        projects
            .iter()
            .map(|project| {
                (
                    project.project_id.clone(),
                    state
                        .observed
                        .get(&project.root_path)
                        .map_or_else(RecentProjectIcon::default, |observed| observed.icon.clone()),
                )
            })
            .collect()
    }

    /// Resolves every project's subtitle from its cached observation, and
    /// starts one background refresh for roots that are due. Roots of
    /// projects no longer attached leave the cache.
    #[must_use]
    pub fn subtitles(
        &self,
        projects: &[ProjectSummary],
        notifier: &ConversationCommitNotifier,
    ) -> HashMap<ProjectId, DisplayName> {
        let subtitles = {
            let state = self.lock();
            projects
                .iter()
                .map(|project| {
                    let observation = state
                        .observed
                        .get(&project.root_path)
                        .and_then(|observed| observed.observation.as_ref());
                    (
                        project.project_id.clone(),
                        project_subtitle(&project.display_name, observation),
                    )
                })
                .collect::<HashMap<_, _>>()
        };
        if let Some((roots, slot)) = self.claim_due(projects)
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            let notifier = notifier.clone();
            runtime.spawn(async move { slot.0.refresh(roots, &notifier).await });
        }
        subtitles
    }

    /// Keeps every attached project's observation current until `cancel`
    /// fires, so subtitles resolve after startup without waiting for a
    /// listing to ask, and a failed read is retried rather than kept.
    pub async fn keep_current(
        self,
        repository: Repository,
        notifier: ConversationCommitNotifier,
        cancel: Arc<CancelHandle>,
    ) {
        loop {
            // An unreadable catalog is read again like a failed root.
            let mut wait = FAILED_OBSERVATION_RETRY;
            if let Ok(listing) = repository.list_projects().await {
                if let Some((roots, slot)) = self.claim_due(listing.projects()) {
                    slot.0.refresh(roots, &notifier).await;
                }
                wait = wait.max(self.next_due_in(listing.projects()));
            }
            tokio::select! {
                () = cancel.wait() => return,
                () = tokio::time::sleep(wait) => {}
            }
        }
    }

    /// Drops roots no longer attached and, unless a refresh is running,
    /// claims the refresh slot for the roots that are due.
    fn claim_due(&self, projects: &[ProjectSummary]) -> Option<(Vec<RootPath>, RefreshSlot)> {
        let now = Instant::now();
        let mut state = self.lock();
        state
            .observed
            .retain(|root, _| projects.iter().any(|project| &project.root_path == root));
        if state.refreshing {
            return None;
        }
        let due = projects
            .iter()
            .filter(|project| {
                state
                    .observed
                    .get(&project.root_path)
                    .is_none_or(|observed| observed.due() <= now)
            })
            .map(|project| project.root_path.clone())
            .collect::<Vec<_>>();
        if due.is_empty() {
            return None;
        }
        state.refreshing = true;
        Some((due, RefreshSlot(self.clone())))
    }

    /// How long until the first of `projects` is due.
    fn next_due_in(&self, projects: &[ProjectSummary]) -> Duration {
        let now = Instant::now();
        let state = self.lock();
        projects
            .iter()
            .map(|project| {
                state
                    .observed
                    .get(&project.root_path)
                    .map_or(Duration::ZERO, |observed| {
                        observed.due().saturating_duration_since(now)
                    })
            })
            .min()
            .unwrap_or(OBSERVATION_FRESHNESS)
    }

    /// Observes `roots` in order within the query ceiling, then publishes.
    async fn refresh(&self, roots: Vec<RootPath>, notifier: &ConversationCommitNotifier) {
        let deadline = tokio::time::Instant::now() + PROJECT_REPOSITORY_QUERY_TIMEOUT;
        let mut changed = false;
        for root in roots {
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            let observation = self.shared.service.observe_root(&root).await.ok();
            changed |= self.record(root.clone(), observation);
            self.refresh_icon(root, notifier.clone());
        }
        if changed {
            self.shared.generation.fetch_add(1, Ordering::AcqRel);
            notifier.wake_any();
        }
    }

    /// Stores one observation; true when it differs from the one it replaces.
    fn record(&self, root: RootPath, observation: Option<RepositoryObservation>) -> bool {
        let mut state = self.lock();
        let previous = state.observed.get(&root);
        let changed = previous.is_none_or(|previous| previous.observation != observation);
        let failures = match (&observation, previous) {
            (Some(_), _) => 0,
            (None, previous) => previous.map_or(0, |previous| previous.failures) + 1,
        };
        let icon_identity = crate::project_icon_service::identity(observation.as_ref());
        let retained = previous.filter(|previous| previous.icon_identity == icon_identity);
        let icon = retained.map_or_else(
            || crate::project_icon_service::fallback(icon_identity.as_deref()),
            |previous| previous.icon.clone(),
        );
        let icon_at = retained.and_then(|previous| previous.icon_at);
        let icon_pending = retained.is_some_and(|previous| previous.icon_pending);
        state.observed.insert(
            root,
            Observed {
                observation,
                at: Instant::now(),
                failures,
                icon,
                icon_identity,
                icon_at,
                icon_pending,
            },
        );
        changed
    }

    fn refresh_icon(&self, root: RootPath, notifier: ConversationCommitNotifier) {
        let identity = {
            let mut state = self.lock();
            let Some(observed) = state.observed.get_mut(&root) else {
                return;
            };
            let Some(identity) = observed.icon_identity.clone() else {
                return;
            };
            let freshness = if observed.icon.png.is_empty() {
                Duration::from_secs(300)
            } else {
                Duration::from_secs(3600)
            };
            if observed.icon_pending || observed.icon_at.is_some_and(|at| at.elapsed() < freshness)
            {
                return;
            }
            observed.icon_pending = true;
            identity
        };
        let subtitles = self.clone();
        tokio::spawn(async move {
            let icon = crate::project_icon_service::resolve(&root, &identity).await;
            let changed = {
                let mut state = subtitles.lock();
                let Some(observed) = state.observed.get_mut(&root) else {
                    return;
                };
                if observed.icon_identity.as_ref() != Some(&identity) {
                    return;
                }
                observed.icon_pending = false;
                observed.icon_at = Some(Instant::now());
                let changed = observed.icon != icon;
                observed.icon = icon;
                changed
            };
            if changed {
                subtitles.shared.generation.fetch_add(1, Ordering::AcqRel);
                notifier.wake_any();
            }
        });
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, CacheState> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}
