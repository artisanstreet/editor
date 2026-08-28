//! Attached-project catalog rediscovery.

use artisan_domain::{PROJECT_LISTING_MAX_PROJECTS, ProjectListing};
use sea_orm::{EntityTrait, QueryOrder, QuerySelect};

use crate::entities;

use super::project_threads::project_summary;
use super::{Repository, RepositoryError, database_error};

impl Repository {
    /// Lists every attached project by deterministic attachment recency.
    ///
    /// The query reads at most one row beyond the domain ceiling so an
    /// oversized catalog becomes a typed error without loading an unbounded
    /// table into memory.
    ///
    /// # Errors
    ///
    /// Returns a typed bounded-listing overflow, corrupt persisted data, or a
    /// preserved database failure.
    pub async fn list_projects(&self) -> Result<ProjectListing, RepositoryError> {
        let rows = entities::attached_project::Entity::find()
            .order_by_desc(entities::attached_project::Column::AttachedAtMs)
            .order_by_asc(entities::attached_project::Column::ProjectId)
            .limit((PROJECT_LISTING_MAX_PROJECTS + 1) as u64)
            .all(&self.database)
            .await
            .map_err(|source| database_error("list attached projects", source))?;
        let summaries = rows
            .into_iter()
            .map(project_summary)
            .collect::<Result<Vec<_>, _>>()?;

        ProjectListing::new(summaries).map_err(|source| RepositoryError::ProjectListing { source })
    }
}
