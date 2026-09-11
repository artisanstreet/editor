//! Owned project-repository facts for the native protocol.
//!
//! This is the owned counterpart of `modules/protocol/src/repository.ts`: a
//! project's identity — where `HEAD` sits and where the checkout publishes —
//! without any working-tree content. The encoding and decoding functions in
//! [`crate::codec`] build these values only after every bounded field has
//! passed the validation here, and the backend fills them from a bounded Git
//! read of the attached project root.
//!
//! Nothing in this module touches the filesystem, spawns Git, or parses remote
//! URLs: host classification and browser projection belong to the backend's
//! Git remote policy.

#![allow(clippy::module_name_repetitions)]
#![forbid(unsafe_code)]

use std::fmt;

use artisan_domain::ProjectId;

use crate::types::ProtocolValueError;

/// Maximum number of projects one repository query or result may name.
pub const PROJECT_REPOSITORY_MAXIMUM_PROJECTS: usize = 128;

/// Maximum number of configured remotes retained for one repository.
pub const REPOSITORY_REMOTE_MAXIMUM: usize = 64;

/// Maximum UTF-8 byte length of one repository name or URL field.
pub const REPOSITORY_TEXT_MAX_BYTES: usize = 2_048;

/// Names the hosting service a remote points at.
///
/// Detection is structural — it reads the remote's host name — so a
/// self-hosted GitLab or Gitea is recognised the same way the public services
/// are. [`RepositoryHost::Other`] covers a reachable web host that matches no
/// known service; [`RepositoryHost::Unknown`] covers a remote whose URL
/// carries no usable host at all, such as a bare filesystem path.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RepositoryHost {
    /// Azure DevOps.
    Azure,
    /// Bitbucket.
    Bitbucket,
    /// Codeberg.
    Codeberg,
    /// Gitea, including self-hosted names containing a `gitea.` label.
    Gitea,
    /// GitHub.
    GitHub,
    /// GitLab, including self-hosted names containing a `gitlab.` label.
    GitLab,
    /// A reachable network host with no recognized service family.
    Other,
    /// `SourceHut`.
    Sourcehut,
    /// No usable network hostname was present.
    Unknown,
}

impl RepositoryHost {
    /// Every host in the protocol's literal vocabulary, in its source order.
    pub const ALL: [Self; 9] = [
        Self::Azure,
        Self::Bitbucket,
        Self::Codeberg,
        Self::Gitea,
        Self::GitHub,
        Self::GitLab,
        Self::Other,
        Self::Sourcehut,
        Self::Unknown,
    ];

    /// Returns the exact lower-case protocol spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Azure => "azure",
            Self::Bitbucket => "bitbucket",
            Self::Codeberg => "codeberg",
            Self::Gitea => "gitea",
            Self::GitHub => "github",
            Self::GitLab => "gitlab",
            Self::Other => "other",
            Self::Sourcehut => "sourcehut",
            Self::Unknown => "unknown",
        }
    }

    /// Parses one exact protocol spelling.
    ///
    /// Unknown spellings return `None` so a decoder can reject a discriminant
    /// outside this revision instead of guessing at a fallback host.
    #[must_use]
    pub fn from_protocol_spelling(value: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|host| host.as_str() == value)
    }
}

impl fmt::Display for RepositoryHost {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The branch `HEAD` names, with its unborn and detached states preserved.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum RepositoryBranchState {
    /// An attached branch with its exact name.
    Attached {
        /// Exact attached branch name.
        name: String,
    },
    /// A detached `HEAD`.
    Detached,
    /// A symbolic `HEAD` naming a branch that holds no commit yet.
    Unborn {
        /// Exact unborn branch name.
        name: String,
    },
}

impl RepositoryBranchState {
    /// Creates an attached branch after validating its name.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolValueError::Repository`] when the name is empty,
    /// exceeds [`REPOSITORY_TEXT_MAX_BYTES`], or contains control characters.
    pub fn attached(name: impl Into<String>) -> Result<Self, ProtocolValueError> {
        let name = name.into();
        validate_repository_text(&name)?;
        Ok(Self::Attached { name })
    }

    /// Creates an unborn branch after validating its name.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolValueError::Repository`] under the same name rules as
    /// [`Self::attached`].
    pub fn unborn(name: impl Into<String>) -> Result<Self, ProtocolValueError> {
        let name = name.into();
        validate_repository_text(&name)?;
        Ok(Self::Unborn { name })
    }

    /// Creates a detached branch state.
    #[must_use]
    pub const fn detached() -> Self {
        Self::Detached
    }

    /// Returns the exact branch name for attached and unborn branches.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        match self {
            Self::Attached { name } | Self::Unborn { name } => Some(name),
            Self::Detached => None,
        }
    }
}

/// One configured remote, projected for presentation.
///
/// `url` is the remote exactly as Git reports it; `web_url` is present only
/// when that URL resolves to an `https` page a browser can open. Both values
/// are derived by the backend's Git remote policy before they cross the wire.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct RepositoryRemote {
    host: RepositoryHost,
    name: String,
    url: String,
    web_url: Option<String>,
}

impl RepositoryRemote {
    /// Creates one remote after validating every bounded text field.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolValueError::Repository`] when the name or URL is
    /// empty, exceeds [`REPOSITORY_TEXT_MAX_BYTES`], or contains control
    /// characters, or when a supplied browser URL contains whitespace or
    /// control characters.
    pub fn new(
        host: RepositoryHost,
        name: impl Into<String>,
        url: impl Into<String>,
        web_url: Option<String>,
    ) -> Result<Self, ProtocolValueError> {
        let name = name.into();
        let url = url.into();
        validate_repository_text(&name)?;
        validate_repository_text(&url)?;
        if let Some(web_url) = web_url.as_deref() {
            if web_url.is_empty() || web_url.len() > REPOSITORY_TEXT_MAX_BYTES {
                return Err(ProtocolValueError::Repository {
                    reason: "remote web url is empty or exceeds its byte bound",
                });
            }
            if web_url
                .chars()
                .any(|character| character.is_whitespace() || character.is_control())
            {
                return Err(ProtocolValueError::Repository {
                    reason: "remote web url contains whitespace or control characters",
                });
            }
        }
        Ok(Self {
            host,
            name,
            url,
            web_url,
        })
    }

    /// Returns the hosting service identified from the remote URL.
    #[must_use]
    pub const fn host(&self) -> RepositoryHost {
        self.host
    }

    /// Returns the configured remote name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the remote URL exactly as Git reported it.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Returns the browser-facing URL, when the remote names a web page.
    #[must_use]
    pub fn web_url(&self) -> Option<&str> {
        self.web_url.as_deref()
    }
}

/// One repository's identity: where `HEAD` sits and where it publishes.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct RepositorySnapshot {
    branch: RepositoryBranchState,
    default_remote: Option<String>,
    remotes: Vec<RepositoryRemote>,
}

impl RepositorySnapshot {
    /// Creates one repository identity after validating its invariants.
    ///
    /// The remote a link should target is `origin` when present, otherwise the
    /// first configured remote. A repository with remotes always names a
    /// default; a repository with no remotes names none.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolValueError::Repository`] when the remote list exceeds
    /// [`REPOSITORY_REMOTE_MAXIMUM`], a supplied default remote is invalid or
    /// names no configured remote, or a repository with remotes omits the
    /// default.
    pub fn new(
        branch: RepositoryBranchState,
        default_remote: Option<String>,
        remotes: Vec<RepositoryRemote>,
    ) -> Result<Self, ProtocolValueError> {
        if remotes.len() > REPOSITORY_REMOTE_MAXIMUM {
            return Err(ProtocolValueError::Repository {
                reason: "repository holds more remotes than its bound",
            });
        }
        if let Some(default_remote) = default_remote.as_deref() {
            validate_repository_text(default_remote)?;
            if !remotes.iter().any(|remote| remote.name() == default_remote) {
                return Err(ProtocolValueError::Repository {
                    reason: "default remote does not name a configured remote",
                });
            }
        } else if !remotes.is_empty() {
            return Err(ProtocolValueError::Repository {
                reason: "repository with remotes must name a default remote",
            });
        }
        Ok(Self {
            branch,
            default_remote,
            remotes,
        })
    }

    /// Returns the branch `HEAD` names.
    #[must_use]
    pub const fn branch(&self) -> &RepositoryBranchState {
        &self.branch
    }

    /// Returns the name of the remote a link should target.
    #[must_use]
    pub fn default_remote(&self) -> Option<&str> {
        self.default_remote.as_deref()
    }

    /// Returns the configured remotes in Git's own order.
    #[must_use]
    pub fn remotes(&self) -> &[RepositoryRemote] {
        &self.remotes
    }

    /// Returns the default remote's `origin`-then-first projection.
    ///
    /// This re-applies the selection rule rather than trusting the wire's
    /// `default_remote`; a decoder has already proven both agree.
    #[must_use]
    pub fn default_remote_projection(&self) -> Option<&RepositoryRemote> {
        self.remotes
            .iter()
            .find(|remote| remote.name() == "origin")
            .or_else(|| self.remotes.first())
    }
}

/// Every observation of one project's repository state.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ProjectRepository {
    /// A directory that Git does not track.
    NotRepository,
    /// A repository with its identity facts.
    Repository(RepositorySnapshot),
}

impl ProjectRepository {
    /// Returns the repository identity, when the root is a repository.
    #[must_use]
    pub const fn snapshot(&self) -> Option<&RepositorySnapshot> {
        match self {
            Self::NotRepository => None,
            Self::Repository(snapshot) => Some(snapshot),
        }
    }
}

/// One project paired with the repository observed at its root.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ProjectRepositoryEntry {
    project_id: ProjectId,
    repository: ProjectRepository,
}

impl ProjectRepositoryEntry {
    /// Pairs one project with its observed repository without re-inspecting.
    #[must_use]
    pub const fn new(project_id: ProjectId, repository: ProjectRepository) -> Self {
        Self {
            project_id,
            repository,
        }
    }

    /// Returns the project this observation belongs to.
    #[must_use]
    pub const fn project_id(&self) -> &ProjectId {
        &self.project_id
    }

    /// Returns the repository observed at the project root.
    #[must_use]
    pub const fn repository(&self) -> &ProjectRepository {
        &self.repository
    }
}

/// Requests repository state for named projects.
///
/// An empty `project_ids` asks for every project in the catalog, which is what
/// a picker listing them all wants.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ProjectRepositoryQuery {
    project_ids: Vec<ProjectId>,
}

impl ProjectRepositoryQuery {
    /// Creates a bounded query after checking its identifier count.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolValueError::Repository`] when more than
    /// [`PROJECT_REPOSITORY_MAXIMUM_PROJECTS`] identifiers are supplied.
    pub fn new(project_ids: Vec<ProjectId>) -> Result<Self, ProtocolValueError> {
        if project_ids.len() > PROJECT_REPOSITORY_MAXIMUM_PROJECTS {
            return Err(ProtocolValueError::Repository {
                reason: "repository query names more projects than its bound",
            });
        }
        Ok(Self { project_ids })
    }

    /// Returns the requested project identifiers in their supplied order.
    #[must_use]
    pub fn project_ids(&self) -> &[ProjectId] {
        &self.project_ids
    }
}

/// Returns repository state per project.
///
/// A root that has moved or lost its repository reports
/// [`ProjectRepository::NotRepository`] rather than failing the whole query.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ProjectRepositoryQueryResult {
    repositories: Vec<ProjectRepositoryEntry>,
}

impl ProjectRepositoryQueryResult {
    /// Creates a bounded result after checking its entry count.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolValueError::Repository`] when more than
    /// [`PROJECT_REPOSITORY_MAXIMUM_PROJECTS`] entries are supplied.
    pub fn new(repositories: Vec<ProjectRepositoryEntry>) -> Result<Self, ProtocolValueError> {
        if repositories.len() > PROJECT_REPOSITORY_MAXIMUM_PROJECTS {
            return Err(ProtocolValueError::Repository {
                reason: "repository result holds more projects than its bound",
            });
        }
        Ok(Self { repositories })
    }

    /// Returns the observed entries in the order the producer selected them.
    #[must_use]
    pub fn repositories(&self) -> &[ProjectRepositoryEntry] {
        &self.repositories
    }
}

/// Validates one non-empty bounded repository text field.
fn validate_repository_text(value: &str) -> Result<(), ProtocolValueError> {
    if value.is_empty() {
        return Err(ProtocolValueError::Repository {
            reason: "repository text field is empty",
        });
    }
    if value.len() > REPOSITORY_TEXT_MAX_BYTES {
        return Err(ProtocolValueError::Repository {
            reason: "repository text field exceeds its byte bound",
        });
    }
    if value.chars().any(char::is_control) {
        return Err(ProtocolValueError::Repository {
            reason: "repository text field contains control characters",
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote(name: &str, url: &str) -> RepositoryRemote {
        RepositoryRemote::new(RepositoryHost::GitHub, name, url, None).expect("valid remote")
    }

    #[test]
    fn host_spellings_round_trip_every_vocabulary_entry() {
        for host in RepositoryHost::ALL {
            assert_eq!(
                RepositoryHost::from_protocol_spelling(host.as_str()),
                Some(host)
            );
            assert_eq!(host.to_string(), host.as_str());
        }
        assert_eq!(RepositoryHost::from_protocol_spelling("GitHub"), None);
        assert_eq!(RepositoryHost::from_protocol_spelling(""), None);
    }

    #[test]
    fn branch_states_preserve_names_and_reject_bad_text() {
        assert_eq!(
            RepositoryBranchState::attached("main")
                .expect("attached")
                .name(),
            Some("main")
        );
        assert_eq!(
            RepositoryBranchState::unborn("main")
                .expect("unborn")
                .name(),
            Some("main")
        );
        assert_eq!(RepositoryBranchState::detached().name(), None);
        assert!(RepositoryBranchState::attached("").is_err());
        assert!(RepositoryBranchState::attached("bad\nname").is_err());
        assert!(
            RepositoryBranchState::attached("x".repeat(REPOSITORY_TEXT_MAX_BYTES + 1)).is_err()
        );
    }

    #[test]
    fn remotes_validate_their_bounded_text() {
        let remote = remote("origin", "git@github.com:owner/repository.git");
        assert_eq!(remote.host(), RepositoryHost::GitHub);
        assert_eq!(remote.name(), "origin");
        assert_eq!(remote.url(), "git@github.com:owner/repository.git");
        assert_eq!(remote.web_url(), None);

        assert!(
            RepositoryRemote::new(
                RepositoryHost::GitHub,
                "origin",
                "https://github.com/owner/repository.git",
                Some("https://github.com/owner/repository".to_owned()),
            )
            .expect("browsable remote")
            .web_url()
            .is_some()
        );
        assert!(
            RepositoryRemote::new(
                RepositoryHost::GitHub,
                "",
                "https://github.com/owner/repository.git",
                None,
            )
            .is_err()
        );
        assert!(
            RepositoryRemote::new(
                RepositoryHost::GitHub,
                "origin",
                "https://github.com/owner/repository.git",
                Some("https://github.com/owner repository".to_owned()),
            )
            .is_err()
        );
    }

    #[test]
    fn snapshot_enforces_default_remote_invariants() {
        let snapshot = RepositorySnapshot::new(
            RepositoryBranchState::attached("main").expect("branch"),
            Some("origin".to_owned()),
            vec![remote("origin", "https://github.com/owner/repository.git")],
        )
        .expect("valid snapshot");
        assert_eq!(snapshot.default_remote(), Some("origin"));
        assert_eq!(
            snapshot
                .default_remote_projection()
                .map(RepositoryRemote::name),
            Some("origin")
        );

        assert!(
            RepositorySnapshot::new(
                RepositoryBranchState::detached(),
                Some("origin".to_owned()),
                vec![],
            )
            .is_err(),
            "a named default remote must exist"
        );
        assert!(
            RepositorySnapshot::new(
                RepositoryBranchState::detached(),
                None,
                vec![remote(
                    "upstream",
                    "https://github.com/owner/repository.git"
                )],
            )
            .is_err(),
            "a repository with remotes must name a default"
        );

        let local_only = RepositorySnapshot::new(
            RepositoryBranchState::unborn("main").expect("branch"),
            None,
            vec![],
        )
        .expect("local-only repository");
        assert_eq!(local_only.default_remote(), None);
    }

    #[test]
    fn query_and_result_are_bounded() {
        let query = ProjectRepositoryQuery::new(vec![]).expect("empty query");
        assert!(query.project_ids().is_empty());

        let projects: Vec<ProjectId> = (0..PROJECT_REPOSITORY_MAXIMUM_PROJECTS)
            .map(|index| ProjectId::parse(format!("project-{index}")).expect("project"))
            .collect();
        assert!(ProjectRepositoryQuery::new(projects.clone()).is_ok());
        let mut over = projects;
        over.push(ProjectId::parse("project-over").expect("project"));
        assert!(ProjectRepositoryQuery::new(over).is_err());
    }

    #[test]
    fn entries_pair_identity_and_not_repository_state() {
        let project = ProjectId::parse("project-1").expect("project");
        let entry = ProjectRepositoryEntry::new(project.clone(), ProjectRepository::NotRepository);
        assert_eq!(entry.project_id(), &project);
        assert_eq!(entry.repository(), &ProjectRepository::NotRepository);
        assert_eq!(entry.repository().snapshot(), None);
    }
}
