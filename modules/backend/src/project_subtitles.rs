//! Process-wide subtitles of attached projects for the recent-threads
//! listing.
//!
//! A subtitle derives purely from the latest repository observation of the
//! project's root ([`crate::project_subtitle_policy`]). Observations are
//! cached per root so producing a listing never waits on Git: a root never
//! observed, or observed longer ago than [`OBSERVATION_FRESHNESS`], is
//! re-observed by one bounded background refresh, and the listing uses the
//! display name (or the previous observation) meanwhile. A refresh that
//! changed an observation advances [`ProjectSubtitles::generation`] and wakes
//! every connection, whose delivery then pushes the changed listing.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use artisan_domain::{DisplayName, ProjectId, ProjectSummary, RootPath};

use crate::conversation_commit_notifier::ConversationCommitNotifier;
use crate::project_repository_service::{
    PROJECT_REPOSITORY_QUERY_TIMEOUT, ProjectRepositoryService, RepositoryObservation,
};
use crate::project_subtitle_policy::project_subtitle;

/// How long one observation of a root stands before a listing refreshes it.
pub const OBSERVATION_FRESHNESS: Duration = Duration::from_secs(60);

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

    /// Resolves every project's subtitle from its cached observation, and
    /// starts one background refresh for roots whose observation is missing
    /// or stale. Roots of projects no longer attached leave the cache.
    #[must_use]
    pub fn subtitles(
        &self,
        projects: &[ProjectSummary],
        notifier: &ConversationCommitNotifier,
    ) -> HashMap<ProjectId, DisplayName> {
        let now = Instant::now();
        let (subtitles, stale) = {
            let mut state = self.lock();
            state
                .observed
                .retain(|root, _| projects.iter().any(|project| &project.root_path == root));
            let subtitles = projects
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
                .collect::<HashMap<_, _>>();
            let stale = projects
                .iter()
                .filter(|project| {
                    state
                        .observed
                        .get(&project.root_path)
                        .is_none_or(|observed| {
                            now.saturating_duration_since(observed.at) >= OBSERVATION_FRESHNESS
                        })
                })
                .map(|project| project.root_path.clone())
                .collect::<Vec<_>>();
            let refresh = !stale.is_empty() && !state.refreshing;
            if refresh {
                state.refreshing = true;
            }
            (subtitles, refresh.then_some(stale))
        };
        if let Some(stale) = stale {
            self.spawn_refresh(stale, notifier.clone());
        }
        subtitles
    }

    /// Observes `roots` in order within the query ceiling, then publishes.
    fn spawn_refresh(&self, roots: Vec<RootPath>, notifier: ConversationCommitNotifier) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            self.lock().refreshing = false;
            return;
        };
        let subtitles = self.clone();
        runtime.spawn(async move {
            let deadline = tokio::time::Instant::now() + PROJECT_REPOSITORY_QUERY_TIMEOUT;
            let mut changed = false;
            for root in roots {
                if tokio::time::Instant::now() >= deadline {
                    break;
                }
                let observation = subtitles.shared.service.observe_root(&root).await.ok();
                changed |= subtitles.record(root, observation);
            }
            subtitles.lock().refreshing = false;
            if changed {
                subtitles.shared.generation.fetch_add(1, Ordering::AcqRel);
                notifier.wake_any();
            }
        });
    }

    /// Stores one observation; true when it differs from the one it replaces.
    fn record(&self, root: RootPath, observation: Option<RepositoryObservation>) -> bool {
        let mut state = self.lock();
        let changed = state
            .observed
            .get(&root)
            .is_none_or(|previous| previous.observation != observation);
        state.observed.insert(
            root,
            Observed {
                observation,
                at: Instant::now(),
            },
        );
        changed
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, CacheState> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}
