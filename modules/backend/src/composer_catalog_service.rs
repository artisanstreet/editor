//! Thread-scoped catalog discovery using the dispatcher's single engine owner.
//!
//! This service owns only authoritative scope resolution, bounded owner
//! admission, and a short-lived result cache. Durable favorites are overlaid
//! by the request-handler adapter immediately before a response is encoded,
//! so a catalog cache hit never serves stale preference state.

#![allow(clippy::module_name_repetitions)]

use std::{path::PathBuf, sync::Arc, time::Duration};

use artisan_database::{Repository, RepositoryError};
use artisan_domain::{EngineProfileId, ThreadId};
use artisan_native_engine::NativeOpenCode2Authority;
use thiserror::Error;

use crate::engine_owner::{
    catalog::{CatalogResult, CatalogScope},
    EngineBounds, EngineCatalogClient, EngineCatalogInput, PreflightDeadlines,
};

const CACHE_TTL: Duration = Duration::from_secs(60);
const READINESS_BUDGET: Duration = Duration::from_secs(15);
const HEALTH_BUDGET: Duration = Duration::from_secs(20);
const ADMISSION_BUDGET: Duration = Duration::from_secs(25);
const CLOSE_BUDGET: Duration = Duration::from_secs(30);
const CATALOG_BUDGET: Duration = Duration::from_secs(25);

/// Payload-free failure while resolving or discovering one composer catalog.
///
/// The owner keeps provider, executable, path, and HTTP details private. The
/// request-handler maps these finite categories to the protocol's bounded
/// error vocabulary.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub(crate) enum ComposerCatalogServiceError {
    /// The requested thread does not identify a durable thread.
    #[error("composer catalog thread is unknown")]
    ThreadUnknown,
    /// The thread's attached project is absent.
    #[error("composer catalog project is unknown")]
    ProjectUnknown,
    /// The repository could not read authoritative thread scope.
    #[error("composer catalog scope persistence is unavailable")]
    PersistenceUnavailable,
    /// The requested registered engine profile cannot be certified.
    #[error("composer catalog engine profile is unavailable")]
    ProfileUnavailable,
    /// The authoritative scope could not be constructed.
    #[error("composer catalog scope is unavailable")]
    ScopeUnavailable,
    /// The owner queue has no admission capacity right now.
    #[error("composer catalog owner is busy")]
    Busy,
    /// The owner is unavailable or its admission channel is closed.
    #[error("composer catalog owner is unavailable")]
    Unavailable,
    /// The bounded discovery/cleanup operation exceeded its deadline.
    #[error("composer catalog discovery timed out")]
    TimedOut,
    /// The owner returned a non-successful, payload-free operation result.
    #[error("composer catalog discovery failed")]
    DiscoveryFailed,
    /// The owner returned a result for a different requested scope.
    #[error("composer catalog scope did not match the requested scope")]
    ScopeMismatch,
}

/// Cloneable thread/profile catalog discovery service.
#[derive(Clone)]
pub(crate) struct ComposerCatalogService {
    owner: EngineCatalogClient,
    repository: Repository,
    database: PathBuf,
    cache: Arc<tokio::sync::Mutex<Option<(tokio::time::Instant, CatalogResult)>>>,
}

impl ComposerCatalogService {
    /// Creates a service bound to the process-owned catalog owner and database.
    pub(crate) fn new(
        owner: EngineCatalogClient,
        repository: Repository,
        database: PathBuf,
    ) -> Self {
        Self {
            owner,
            repository,
            database,
            cache: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    /// Discovers the catalog for the exact durable thread root and registered
    /// engine profile.
    ///
    /// The cache lock is held only while checking or replacing the bounded
    /// in-memory entry. In particular, it is released before owner admission,
    /// the 25-second discovery wait, and bounded cleanup. Concurrent misses
    /// may perform independent owner admissions; the last valid result wins.
    ///
    /// # Errors
    ///
    /// Returns a finite [`ComposerCatalogServiceError`] with no path, provider
    /// payload, executable, or process detail.
    pub(crate) async fn discover(
        &self,
        thread: &ThreadId,
        profile: &EngineProfileId,
    ) -> Result<CatalogResult, ComposerCatalogServiceError> {
        let root = self
            .repository
            .read_thread_project_root(thread)
            .await
            .map_err(|error| classify_scope_repository_error(&error))?;
        let authority = NativeOpenCode2Authority::new();
        let launch = authority
            .resolve_profile_launch(&self.database, profile)
            .map_err(|_| ComposerCatalogServiceError::ProfileUnavailable)?;
        let scope = CatalogScope::new(profile.as_str(), root.as_str(), "safe")
            .map_err(|_| ComposerCatalogServiceError::ScopeUnavailable)?;
        let now = tokio::time::Instant::now();

        {
            let cache = self.cache.lock().await;
            if let Some((observed, result)) = cache.as_ref()
                && result.scope == scope && now.duration_since(*observed) < CACHE_TTL {
                    return Ok(result.clone());
                }
        }

        // Discovery is a bounded product operation, independent of a turn's
        // selected model. No cache guard is held across this await.
        let input = EngineCatalogInput {
            project_root: root,
            launch,
            scope: scope.clone(),
            deadlines: PreflightDeadlines {
                readiness: now + READINESS_BUDGET,
                health: now + HEALTH_BUDGET,
                admission: now + ADMISSION_BUDGET,
                close: now + CLOSE_BUDGET,
            },
            catalog_deadline: now + CATALOG_BUDGET,
            bounds: EngineBounds {
                max_json_body: 8 * 1024 * 1024,
                max_sse_line: 64 * 1024,
                max_sse_event: 1024 * 1024,
                max_readiness_line: 8192,
                max_headers: 64,
                max_buf_bytes: 64 * 1024,
                stderr_cap_bytes: 64 * 1024,
                sink_capacity: 32,
                control_capacity: 8,
            },
        };
        let deadline = input.deadlines.admission;
        let accepted = self.owner.admit_catalog(input).map_err(|error| {
            if matches!(
                error,
                crate::engine_owner::operation::LaunchAdmissionError::Busy
            ) {
                ComposerCatalogServiceError::Busy
            } else {
                ComposerCatalogServiceError::Unavailable
            }
        })?;
        let result = tokio::time::timeout_at(deadline, accepted)
            .await
            .map_err(|_| ComposerCatalogServiceError::TimedOut)?
            .map_err(|_| ComposerCatalogServiceError::DiscoveryFailed)?;
        if result.scope != scope {
            return Err(ComposerCatalogServiceError::ScopeMismatch);
        }

        let mut cache = self.cache.lock().await;
        if let Some((observed, cached)) = cache.as_ref()
            && cached.scope == scope
                && tokio::time::Instant::now().duration_since(*observed) < CACHE_TTL
            {
                return Ok(cached.clone());
            }
        *cache = Some((tokio::time::Instant::now(), result.clone()));
        Ok(result)
    }
}

fn classify_scope_repository_error(error: &RepositoryError) -> ComposerCatalogServiceError {
    match error {
        RepositoryError::ThreadNotFound { .. } => ComposerCatalogServiceError::ThreadUnknown,
        RepositoryError::ProjectNotFound { .. } => ComposerCatalogServiceError::ProjectUnknown,
        _ => ComposerCatalogServiceError::PersistenceUnavailable,
    }
}
