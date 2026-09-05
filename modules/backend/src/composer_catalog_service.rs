//! Thread-scoped catalog discovery using the dispatcher's single engine owner.
use crate::engine_owner::{
    EngineBounds, EngineCatalogClient, EngineCatalogInput, PreflightDeadlines,
    catalog::{CatalogResult, CatalogScope},
};
use artisan_database::Repository;
use artisan_domain::{EngineProfileId, ThreadId};
use artisan_native_engine::NativeOpenCode2Authority;
use std::{path::PathBuf, sync::Arc, time::Duration};

#[derive(Clone)]
pub(crate) struct ComposerCatalogService {
    owner: EngineCatalogClient,
    repository: Repository,
    database: PathBuf,
    cache: Arc<tokio::sync::Mutex<Option<(tokio::time::Instant, CatalogResult)>>>,
}
impl ComposerCatalogService {
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
    pub(crate) async fn discover(
        &self,
        thread: &ThreadId,
        profile: &EngineProfileId,
    ) -> Result<CatalogResult, &'static str> {
        let root = self
            .repository
            .read_thread_project_root(thread)
            .await
            .map_err(|_| "project unavailable")?;
        let authority = NativeOpenCode2Authority::new();
        let launch = authority
            .resolve_profile_launch(&self.database, profile)
            .map_err(|_| "engine profile unavailable")?;
        let scope = CatalogScope::new(profile.as_str(), root.as_str(), "safe")
            .map_err(|_| "catalog scope unavailable")?;
        let now = tokio::time::Instant::now();
        let mut cache = self.cache.lock().await;
        if let Some((observed, result)) = cache.as_ref() {
            if result.scope == scope && now.duration_since(*observed) < Duration::from_secs(60) {
                return Ok(result.clone());
            }
        }
        // Discovery is a bounded product operation, independent of a turn's selected model.
        let input = EngineCatalogInput {
            project_root: root,
            launch,
            scope,
            deadlines: PreflightDeadlines {
                readiness: now + Duration::from_secs(15),
                health: now + Duration::from_secs(20),
                admission: now + Duration::from_secs(25),
                close: now + Duration::from_secs(30),
            },
            catalog_deadline: now + Duration::from_secs(25),
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
        let accepted = self
            .owner
            .admit_catalog(input)
            .map_err(|_| "engine is busy")?;
        let result = tokio::time::timeout_at(deadline, accepted)
            .await
            .map_err(|_| "model discovery timed out")?
            .map_err(|_| "model discovery failed")?;
        *cache = Some((tokio::time::Instant::now(), result.clone()));
        Ok(result)
    }
}
