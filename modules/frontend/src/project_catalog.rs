//! Pure recent-project, preferred-project, and monogram policy.
//!
//! This is the dependency-free Rust counterpart of
//! `modules/frontend/src/lib/root/project-catalog.ts`. The types below are
//! deliberately small projections of the protocol rows: catalog ordering
//! needs only a project's identity and update stamp, while thread activity
//! needs its creation stamp, optional message stamp, and optional primary
//! project identity. Protocol conversion, repositories, and rendering stay
//! outside this leaf.

use std::collections::HashMap;

/// The project fields consumed by the recent/preferred catalog policy.
///
/// Other project metadata, such as a display name or root path, is not
/// inspected by this policy and belongs to the caller's full project model.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Project {
    /// Stable project identity used to join projects with assigned threads.
    pub project_id: String,
    /// Catalog-row fallback activity when no assigned thread exists.
    pub updated_at: String,
}

impl Project {
    /// Builds a policy projection from its identity and update stamp.
    #[must_use]
    pub fn new(project_id: impl Into<String>, updated_at: impl Into<String>) -> Self {
        Self {
            project_id: project_id.into(),
            updated_at: updated_at.into(),
        }
    }
}

/// The primary-project reference optionally carried by a thread row.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ProjectRef {
    /// Identity of the project's primary association.
    pub project_id: String,
}

impl ProjectRef {
    /// Builds a primary-project reference from its identity.
    #[must_use]
    pub fn new(project_id: impl Into<String>) -> Self {
        Self {
            project_id: project_id.into(),
        }
    }
}

/// The thread fields consumed by the recent-project policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadRow {
    /// Creation stamp used when no last-message stamp is present.
    pub created_at: String,
    /// Last-message stamp, when the thread has one.
    pub last_message_at: Option<String>,
    /// Optional primary project assignment; unassigned rows do not count.
    pub primary_project: Option<ProjectRef>,
}

impl ThreadRow {
    /// Builds a thread policy projection.
    #[must_use]
    pub fn new(
        created_at: impl Into<String>,
        last_message_at: Option<String>,
        primary_project: Option<ProjectRef>,
    ) -> Self {
        Self {
            created_at: created_at.into(),
            last_message_at,
            primary_project,
        }
    }

    /// Returns the stamp used as this row's project activity.
    #[must_use]
    pub fn activity_timestamp(&self) -> &str {
        self.last_message_at.as_deref().unwrap_or(&self.created_at)
    }
}

/// One project enriched with the activity used to order the catalog.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecentProject {
    /// Newest assigned-thread activity, or the project's update stamp when
    /// no assigned activity exists.
    pub last_message_at: String,
    /// The project represented by this result.
    pub project: Project,
    /// Number of assigned thread rows for this project.
    pub thread_count: usize,
}

#[derive(Debug)]
struct ProjectActivity {
    last_message_at: String,
    thread_count: usize,
}

/// Builds a newest-first project catalog from project and thread rows.
///
/// A thread contributes only when it has a primary project reference. Its
/// activity stamp is `last_message_at` when present and `created_at`
/// otherwise. Counts include every assigned row whose project id matches,
/// while the activity stamp keeps the lexicographically newest ISO timestamp
/// for that id. Projects with no matching activity use `updated_at` and have
/// a zero count.
///
/// The final `sort_by` is stable. Equal timestamps therefore retain the
/// caller's project order, which makes ties deterministic without inventing a
/// second ordering rule that the TypeScript policy does not have.
#[must_use]
pub fn recent_projects(projects: &[Project], threads: &[ThreadRow]) -> Vec<RecentProject> {
    let mut activity = HashMap::<String, ProjectActivity>::new();

    for thread in threads {
        let Some(project) = thread.primary_project.as_ref() else {
            continue;
        };

        let timestamp = thread.activity_timestamp();
        let seen = activity
            .entry(project.project_id.clone())
            .or_insert_with(|| ProjectActivity {
                last_message_at: timestamp.to_owned(),
                thread_count: 0,
            });
        seen.thread_count += 1;
        if timestamp > seen.last_message_at.as_str() {
            timestamp.clone_into(&mut seen.last_message_at);
        }
    }

    let mut recents = projects
        .iter()
        .map(|project| {
            let seen = activity.get(&project.project_id);
            RecentProject {
                last_message_at: seen.map_or_else(
                    || project.updated_at.clone(),
                    |item| item.last_message_at.clone(),
                ),
                project: project.clone(),
                thread_count: seen.map_or(0, |item| item.thread_count),
            }
        })
        .collect::<Vec<_>>();

    recents.sort_by(|left, right| right.last_message_at.cmp(&left.last_message_at));
    recents
}

/// Chooses the requested recent project, falling back to the first recent
/// project and then to no project when the catalog is empty.
#[must_use]
pub fn preferred_project<'a>(
    recents: &'a [RecentProject],
    preferred_project_id: Option<&str>,
) -> Option<&'a Project> {
    preferred_project_id
        .and_then(|project_id| {
            recents
                .iter()
                .find(|recent| recent.project.project_id == project_id)
                .map(|recent| &recent.project)
        })
        .or_else(|| recents.first().map(|recent| &recent.project))
}

/// Returns the preferred project as an owned value for callers that need to
/// retain it after the recent-result slice is released.
#[must_use]
pub fn preferred_project_owned(
    recents: &[RecentProject],
    preferred_project_id: Option<&str>,
) -> Option<Project> {
    preferred_project(recents, preferred_project_id).cloned()
}

/// Placeholder shown when a project name contains no usable parts.
pub const EMPTY_MONOGRAM: &str = "··";

/// Builds the compact project monogram used by a small project tile.
///
/// Names split on hyphen, underscore, period, ASCII space, forward slash, or
/// backslash. Empty parts are discarded. A single remaining part contributes
/// its first two Unicode scalar values; multiple parts contribute the first
/// scalar of each of the first two parts. Each selected scalar is uppercased
/// with Rust's Unicode mapping, including mappings that expand to more than
/// one scalar.
#[must_use]
pub fn project_monogram(name: &str) -> String {
    let parts = name
        .split(['-', '_', '.', ' ', '/', '\\'])
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();

    match parts.as_slice() {
        [] => EMPTY_MONOGRAM.to_owned(),
        [part] => uppercase_first_scalars(part, 2),
        [first, second, ..] => {
            let mut monogram = uppercase_first_scalars(first, 1);
            monogram.push_str(&uppercase_first_scalars(second, 1));
            monogram
        }
    }
}

fn uppercase_first_scalars(value: &str, count: usize) -> String {
    let mut uppercase = String::new();
    for character in value.chars().take(count) {
        uppercase.extend(character.to_uppercase());
    }
    uppercase
}
