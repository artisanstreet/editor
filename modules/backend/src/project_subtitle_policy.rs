//! The line the Editor shows beneath a thread's title: which repository, or
//! which project, the thread works in.
//!
//! A project whose root is a Git repository with a default remote names that
//! repository by its path on the host (`owner/repo`, the full group path for
//! nested namespaces; the host itself is omitted, self-hosted ones too). A
//! linked worktree appends its branch (or, detached, its short commit or its
//! directory name) so several checkouts of one repository stay apart. A root
//! without remotes, outside Git, or whose state is unknown shows the
//! project's display name.

#![forbid(unsafe_code)]

use artisan_domain::DisplayName;
use artisan_protocol::ProjectRepository;

use crate::git_remote_url_policy::repository_path_for;
use crate::project_repository_service::{LinkedWorktree, RepositoryObservation};

/// Separates the repository from a linked worktree's branch.
pub const WORKTREE_SEPARATOR: &str = " · ";

/// Resolves the subtitle of a project from its latest repository
/// observation; `None` means the root has not been observed or could not be
/// read.
#[must_use]
pub fn project_subtitle(
    display_name: &DisplayName,
    observation: Option<&RepositoryObservation>,
) -> DisplayName {
    observation
        .and_then(repository_subtitle)
        .and_then(|subtitle| DisplayName::parse(subtitle).ok())
        .unwrap_or_else(|| display_name.clone())
}

fn repository_subtitle(observation: &RepositoryObservation) -> Option<String> {
    let ProjectRepository::Repository(snapshot) = &observation.repository else {
        return None;
    };
    let path = repository_path_for(snapshot.default_remote_projection()?.url())?;
    Some(match &observation.linked_worktree {
        None => path,
        Some(worktree) => {
            let checkout = snapshot
                .branch()
                .name()
                .map_or_else(|| detached_label(worktree), str::to_owned);
            format!("{path}{WORKTREE_SEPARATOR}{checkout}")
        }
    })
}

fn detached_label(worktree: &LinkedWorktree) -> String {
    worktree
        .short_head
        .clone()
        .unwrap_or_else(|| worktree.directory_name.clone())
}
