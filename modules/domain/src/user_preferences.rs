//! The Forge user's preferences, navigation record, and account profile.
//!
//! A Forge serves one account (the host user it runs as), so these values
//! are that user's: the default engine configuration new threads start
//! from, the projects in most-recently-used order with the thread last open
//! in each, and the route (project and thread) the user was last on, so a
//! restarted or reconnected Editor resumes where the user was. The Editor
//! renders them and reports navigation; the Forge owns every rule (the
//! most-recently-used order, which thread is remembered, when a saved
//! configuration becomes the default).

use thiserror::Error;

use crate::bounds::PROJECT_LISTING_MAX_PROJECTS;
use crate::catalog_selection::CatalogSelection;
use crate::engine_config::EngineRunConfig;
use crate::identifiers::{ProjectId, RequestId, ThreadId};
use crate::text::DisplayName;

/// Maximum number of projects in a navigation record or a legacy import.
pub const NAVIGATION_PROJECTS_MAX: usize = PROJECT_LISTING_MAX_PROJECTS;

/// One project of the navigation record and the thread last open in it.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct NavigationProject {
    /// The project.
    pub project_id: ProjectId,
    /// The thread the user last had open in it, if any.
    pub last_thread_id: Option<ThreadId>,
}

/// Where the user last was: a project and, when one was open, its thread.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct NavigationRoute {
    /// Selected project.
    pub project_id: ProjectId,
    /// Open thread of that project, if any.
    pub thread_id: Option<ThreadId>,
}

/// The user's navigation record.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NavigationRecord {
    projects: Vec<NavigationProject>,
    route: Option<NavigationRoute>,
}

/// Invalid navigation record.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum NavigationRecordError {
    /// More projects than [`NAVIGATION_PROJECTS_MAX`].
    #[error("navigation record names more than {NAVIGATION_PROJECTS_MAX} projects")]
    TooManyProjects,
    /// A project appears twice.
    #[error("navigation record names a project twice")]
    DuplicateProject,
}

impl NavigationRecord {
    /// Creates a record from projects in most-recently-used order.
    ///
    /// # Errors
    ///
    /// Returns [`NavigationRecordError`] for too many or repeated projects.
    pub fn new(
        projects: Vec<NavigationProject>,
        route: Option<NavigationRoute>,
    ) -> Result<Self, NavigationRecordError> {
        if projects.len() > NAVIGATION_PROJECTS_MAX {
            return Err(NavigationRecordError::TooManyProjects);
        }
        let mut seen = std::collections::HashSet::with_capacity(projects.len());
        if !projects
            .iter()
            .all(|project| seen.insert(&project.project_id))
        {
            return Err(NavigationRecordError::DuplicateProject);
        }
        Ok(Self { projects, route })
    }

    /// Projects, most recently used first.
    #[must_use]
    pub fn projects(&self) -> &[NavigationProject] {
        &self.projects
    }

    /// The route the user was last on.
    #[must_use]
    pub const fn route(&self) -> Option<&NavigationRoute> {
        self.route.as_ref()
    }

    /// The thread last open in `project`.
    #[must_use]
    pub fn last_thread(&self, project: &ProjectId) -> Option<&ThreadId> {
        self.projects
            .iter()
            .find(|entry| &entry.project_id == project)
            .and_then(|entry| entry.last_thread_id.as_ref())
    }
}

/// The host account the Forge runs as, for presentation.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct AccountProfile {
    /// The account's display name.
    pub display_name: DisplayName,
    /// The host's name.
    pub host_name: DisplayName,
}

/// Monotonic revision of the stored preferences.
pub type UserPreferencesRevision = u64;

/// Everything the Forge keeps for its user, as one value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UserPreferences {
    /// Revision of the stored preferences; grows with every change.
    pub revision: UserPreferencesRevision,
    /// Configuration new threads start from, once the user chose one.
    pub default_engine_config: Option<EngineRunConfig>,
    /// Project order, last threads, and last route.
    pub navigation: NavigationRecord,
    /// The account the Forge runs as.
    pub account: AccountProfile,
}

/// Reads the user's preferences.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct ReadUserPreferences;

/// Reports that the user opened a project, and a thread in it when one is
/// open. The Forge moves the project to the front, remembers the thread, and
/// records the route; the answer is the resulting preferences.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct RecordNavigation {
    /// Client-minted request identity.
    pub request_id: RequestId,
    /// Opened project.
    pub project_id: ProjectId,
    /// Opened thread of that project, if any.
    pub thread_id: Option<ThreadId>,
}

/// Preferences an older Editor kept in its own files, sent once so the
/// Forge can adopt what it does not have yet.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ImportLegacyPreferences {
    /// Client-minted request identity.
    pub request_id: RequestId,
    /// The last-used model, as catalog identities.
    pub default_selection: Option<CatalogSelection>,
    /// Project order, most recent first.
    pub project_order: Vec<ProjectId>,
}

/// What the Forge did with one imported legacy preference.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LegacyImportOutcome {
    /// Nothing was sent.
    Absent,
    /// The Forge adopted it.
    Imported,
    /// The Forge already had its own and kept it.
    Kept,
    /// The Forge could not use it (for example a model its catalog lacks).
    Refused,
}

/// The Forge's answer to [`ImportLegacyPreferences`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LegacyPreferencesImported {
    /// Outcome for the default model.
    pub default_model: LegacyImportOutcome,
    /// Outcome for the project order.
    pub project_order: LegacyImportOutcome,
    /// Preferences after the import.
    pub preferences: UserPreferences,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(value: &str) -> ProjectId {
        ProjectId::parse(value).expect("project id")
    }

    #[test]
    fn navigation_record_rejects_repeated_and_excess_projects() {
        let entry = |id: &str| NavigationProject {
            project_id: project(id),
            last_thread_id: None,
        };
        assert_eq!(
            NavigationRecord::new(vec![entry("a"), entry("a")], None),
            Err(NavigationRecordError::DuplicateProject)
        );
        let many = (0..=NAVIGATION_PROJECTS_MAX)
            .map(|index| entry(&format!("p{index}")))
            .collect();
        assert_eq!(
            NavigationRecord::new(many, None),
            Err(NavigationRecordError::TooManyProjects)
        );
        let thread = ThreadId::parse("t").expect("thread id");
        let record = NavigationRecord::new(
            vec![NavigationProject {
                project_id: project("a"),
                last_thread_id: Some(thread.clone()),
            }],
            None,
        )
        .expect("record");
        assert_eq!(record.last_thread(&project("a")), Some(&thread));
        assert_eq!(record.last_thread(&project("b")), None);
    }
}
