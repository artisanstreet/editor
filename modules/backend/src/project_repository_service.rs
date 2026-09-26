//! Bounded Git repository-identity reads for attached project roots.
//!
//! This is the native counterpart of `git/repository-service.ts`'s `Inspect`:
//! for one already-attached project it answers "what repository is this
//! directory, and where does it publish". Each read runs Git against the
//! stored root only, with a total deadline and bounded output; a root that is
//! not a repository, moved away, or unreadable reports
//! [`artisan_protocol::ProjectRepository::NotRepository`] rather than failing
//! the whole query.
//!
//! The projected fields intentionally stop at what the reference header and
//! environment card consume: repository state, branch state, and the
//! configured remotes with their browser projections. `head` and the
//! working-tree diff facts stay out of this read so a titlebar decoration
//! never pays for a status walk.
//!
//! Host classification and browser projection reuse
//! [`crate::git_remote_url_policy`] exactly, so an `ssh` or `scp`-style remote
//! is translated the same way the TypeScript service translates it.

#![allow(clippy::module_name_repetitions)]
#![forbid(unsafe_code)]

use std::{path::PathBuf, process::Stdio, time::Duration};

use artisan_database::{Repository, RepositoryError};
use artisan_domain::{ProjectId, RootPath};
use artisan_protocol::{
    PROJECT_REPOSITORY_MAXIMUM_PROJECTS, ProjectRepository, ProjectRepositoryEntry,
    RepositoryBranchState, RepositoryHost, RepositoryRemote, RepositorySnapshot,
};
use tokio::{io::AsyncReadExt, process::Command, time::timeout};

use crate::git_remote_url_policy::{self, RepositoryHost as PolicyRepositoryHost};

/// Default total deadline for one project's Git identity read.
pub const PROJECT_REPOSITORY_READ_TIMEOUT: Duration = Duration::from_secs(10);

/// Default wall-clock ceiling for one whole repository query.
///
/// A picker may ask for every attached project at once; this keeps the whole
/// answer bounded even when several roots are slow. Entries inspected before
/// the ceiling are retained, and the caller still receives a correlated
/// result rather than a failure.
pub const PROJECT_REPOSITORY_QUERY_TIMEOUT: Duration = Duration::from_secs(15);

/// Maximum stdout bytes retained from one Git read.
pub const PROJECT_REPOSITORY_MAX_STDOUT_BYTES: usize = 256 * 1024;

/// Maximum stderr bytes retained from one Git read.
const MAX_STDERR_BYTES: usize = 16 * 1024;

/// Maximum configured remotes retained for one repository.
const MAXIMUM_REMOTES: usize = artisan_protocol::REPOSITORY_REMOTE_MAXIMUM;

/// Finite, payload-free failure for one bounded repository read.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum GitReadError {
    /// The Git executable could not be started or its pipes could not be read.
    #[error("git repository read is unavailable")]
    Unavailable,
    /// The bounded read deadline elapsed.
    #[error("git repository read timed out")]
    Timeout,
}

/// Finite, payload-free failure for a whole project-repository query.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ProjectRepositoryServiceError {
    /// The durable project catalog could not be read.
    #[error("project repository catalog is unavailable")]
    CatalogUnavailable,
}

/// One configured remote exactly as Git reported it, before policy projection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfiguredRemote {
    /// Configured remote name.
    pub name: String,
    /// Remote URL exactly as Git reported it.
    pub url: String,
}

/// One observation of a project root: its repository identity, and whether
/// the root is a linked worktree rather than the repository's main checkout.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryObservation {
    /// The repository identity [`ProjectRepositoryService::inspect_root`]
    /// reports.
    pub repository: ProjectRepository,
    /// Present when the root is a linked worktree (`git worktree add`).
    pub linked_worktree: Option<LinkedWorktree>,
}

/// A linked worktree's own identity beside its repository's.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LinkedWorktree {
    /// The worktree's directory name.
    pub directory_name: String,
    /// HEAD's abbreviated commit, when HEAD names one.
    pub short_head: Option<String>,
}

/// Abbreviated commit length used for a detached worktree.
const SHORT_HEAD_LENGTH: usize = 7;

/// Classifies `git rev-parse --path-format=absolute --git-dir
/// --git-common-dir --show-toplevel` output: a git directory that differs
/// from the common directory belongs to a linked worktree.
#[must_use]
pub fn linked_worktree(rev_parse: &str, head: &str) -> Option<LinkedWorktree> {
    let mut lines = rev_parse.lines().map(str::trim);
    let (git_dir, common_dir, top_level) = (lines.next()?, lines.next()?, lines.next()?);
    if git_dir.is_empty() || git_dir == common_dir {
        return None;
    }
    let directory_name = top_level
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())?
        .to_owned();
    let short_head = (!head.is_empty()).then(|| head.chars().take(SHORT_HEAD_LENGTH).collect());
    Some(LinkedWorktree {
        directory_name,
        short_head,
    })
}

/// Parses `git config --get-regexp` output into configured remotes.
///
/// Each line is `remote.<name>.url <url>`; the first space separates the key
/// from the URL, and a name is retained at most once in Git's own order, so a
/// multi-valued key cannot shadow the remote the caller already saw.
#[must_use]
pub fn parse_configured_remotes(output: &str) -> Vec<ConfiguredRemote> {
    let mut remotes = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for line in output.lines() {
        let Some(separator) = line.find(' ') else {
            continue;
        };
        let key = &line[..separator];
        let url = line[separator + 1..].trim();
        let Some(name) = key
            .strip_prefix("remote.")
            .and_then(|rest| rest.strip_suffix(".url"))
        else {
            continue;
        };

        if name.is_empty() || url.is_empty() || !seen.insert(name.to_owned()) {
            continue;
        }
        if remotes.len() >= MAXIMUM_REMOTES {
            break;
        }

        remotes.push(ConfiguredRemote {
            name: name.to_owned(),
            url: url.to_owned(),
        });
    }

    remotes
}

/// Projects one configured remote through the shared remote policy.
///
/// Returns `None` when policy-derived text fails the protocol's bounded-text
/// validation; the caller drops that remote instead of failing the read.
#[must_use]
pub fn project_remote(remote: &ConfiguredRemote) -> Option<RepositoryRemote> {
    let web_url = git_remote_url_policy::repository_web_url_for(&remote.url);
    RepositoryRemote::new(
        project_host(git_remote_url_policy::repository_host_for(&remote.url)),
        remote.name.clone(),
        remote.url.clone(),
        web_url,
    )
    .ok()
}

/// Maps the backend's remote-host vocabulary onto the protocol's.
#[must_use]
pub const fn project_host(host: PolicyRepositoryHost) -> RepositoryHost {
    match host {
        PolicyRepositoryHost::Azure => RepositoryHost::Azure,
        PolicyRepositoryHost::Bitbucket => RepositoryHost::Bitbucket,
        PolicyRepositoryHost::Codeberg => RepositoryHost::Codeberg,
        PolicyRepositoryHost::Gitea => RepositoryHost::Gitea,
        PolicyRepositoryHost::Github => RepositoryHost::GitHub,
        PolicyRepositoryHost::Gitlab => RepositoryHost::GitLab,
        PolicyRepositoryHost::Other => RepositoryHost::Other,
        PolicyRepositoryHost::Sourcehut => RepositoryHost::Sourcehut,
        PolicyRepositoryHost::Unknown => RepositoryHost::Unknown,
    }
}

/// Selects the remote a link should target: `origin`, then Git's own order.
#[must_use]
pub fn default_remote_name(remotes: &[RepositoryRemote]) -> Option<String> {
    remotes
        .iter()
        .find(|remote| remote.name() == "origin")
        .or_else(|| remotes.first())
        .map(|remote| remote.name().to_owned())
}

/// Reads repository identity for attached projects.
///
/// The service owns only the durable catalog and the bounded Git executable
/// invocation; it persists nothing and holds no inspection state.
#[derive(Clone)]
pub struct ProjectRepositoryService {
    repository: Repository,
    git_executable: PathBuf,
    read_timeout: Duration,
    query_timeout: Duration,
}

impl std::fmt::Debug for ProjectRepositoryService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ProjectRepositoryService { <payload-free> }")
    }
}

impl ProjectRepositoryService {
    /// Creates a service reading through the `git` on `PATH`.
    #[must_use]
    pub fn new(repository: Repository) -> Self {
        Self {
            repository,
            git_executable: PathBuf::from("git"),
            read_timeout: PROJECT_REPOSITORY_READ_TIMEOUT,
            query_timeout: PROJECT_REPOSITORY_QUERY_TIMEOUT,
        }
    }

    /// Overrides the Git executable used by tests and certified launches.
    #[must_use]
    pub fn with_git_executable(mut self, executable: impl Into<PathBuf>) -> Self {
        self.git_executable = executable.into();
        self
    }

    /// Overrides the total deadline for one project's read.
    #[must_use]
    pub fn with_read_timeout(mut self, read_timeout: Duration) -> Self {
        self.read_timeout = read_timeout;
        self
    }

    /// Overrides the wall-clock ceiling for one whole query.
    #[must_use]
    pub fn with_query_timeout(mut self, query_timeout: Duration) -> Self {
        self.query_timeout = query_timeout;
        self
    }

    /// Inspects the named projects in durable catalog order.
    ///
    /// An empty `project_ids` selects every attached project, bounded to
    /// [`PROJECT_REPOSITORY_MAXIMUM_PROJECTS`] and to
    /// [`PROJECT_REPOSITORY_QUERY_TIMEOUT`] wall time. Identifiers absent from
    /// the catalog are omitted, exactly as the reference picker query omits
    /// them.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectRepositoryServiceError::CatalogUnavailable`] when the
    /// durable catalog cannot be read. Per-project Git failures are reported
    /// as [`ProjectRepository::NotRepository`], never as a query failure.
    pub async fn inspect(
        &self,
        project_ids: &[ProjectId],
    ) -> Result<Vec<ProjectRepositoryEntry>, ProjectRepositoryServiceError> {
        let listing = self
            .repository
            .list_projects()
            .await
            .map_err(classify_catalog_error)?;
        let deadline = tokio::time::Instant::now() + self.query_timeout;
        let mut entries = Vec::new();
        for project in listing.projects() {
            if entries.len() >= PROJECT_REPOSITORY_MAXIMUM_PROJECTS {
                break;
            }
            if !project_ids.is_empty() && !project_ids.contains(&project.project_id) {
                continue;
            }
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            let repository = match self.inspect_root(&project.root_path).await {
                Ok(repository) => repository,
                Err(_) => ProjectRepository::NotRepository,
            };
            entries.push(ProjectRepositoryEntry::new(
                project.project_id.clone(),
                repository,
            ));
        }
        Ok(entries)
    }

    /// Inspects one already-attached project root.
    ///
    /// # Errors
    ///
    /// Returns [`GitReadError`] when Git cannot run or the read deadline
    /// elapses; a directory Git does not track is
    /// [`ProjectRepository::NotRepository`] and is not an error.
    pub async fn inspect_root(&self, root: &RootPath) -> Result<ProjectRepository, GitReadError> {
        self.observe(root, false)
            .await
            .map(|observation| observation.repository)
    }

    /// Observes one already-attached project root: its repository identity
    /// and whether it is a linked worktree.
    ///
    /// # Errors
    ///
    /// Returns [`GitReadError`] when Git cannot run or the read deadline
    /// elapses.
    pub async fn observe_root(
        &self,
        root: &RootPath,
    ) -> Result<RepositoryObservation, GitReadError> {
        self.observe(root, true).await
    }

    async fn observe(
        &self,
        root: &RootPath,
        worktree: bool,
    ) -> Result<RepositoryObservation, GitReadError> {
        match timeout(self.read_timeout, self.observe_inner(root, worktree)).await {
            Ok(result) => result,
            Err(_) => Err(GitReadError::Timeout),
        }
    }

    async fn observe_inner(
        &self,
        root: &RootPath,
        worktree: bool,
    ) -> Result<RepositoryObservation, GitReadError> {
        let inside = self
            .run_git(
                root,
                &["rev-parse", "--is-inside-work-tree"],
                PROJECT_REPOSITORY_MAX_STDOUT_BYTES,
            )
            .await?;
        if inside.exit_code != 0 || inside.stdout.trim() != "true" {
            return Ok(RepositoryObservation {
                repository: ProjectRepository::NotRepository,
                linked_worktree: None,
            });
        }

        let (head_ref, head_object, configured, directories) = tokio::join!(
            self.run_git(
                root,
                &["symbolic-ref", "--quiet", "--short", "HEAD"],
                PROJECT_REPOSITORY_MAX_STDOUT_BYTES,
            ),
            self.run_git(
                root,
                &["rev-parse", "--verify", "--quiet", "HEAD"],
                PROJECT_REPOSITORY_MAX_STDOUT_BYTES,
            ),
            self.run_git(
                root,
                &["config", "--get-regexp", r"^remote\..*\.url$"],
                PROJECT_REPOSITORY_MAX_STDOUT_BYTES,
            ),
            async {
                if worktree {
                    self.run_git(
                        root,
                        &[
                            "rev-parse",
                            "--path-format=absolute",
                            "--git-dir",
                            "--git-common-dir",
                            "--show-toplevel",
                        ],
                        PROJECT_REPOSITORY_MAX_STDOUT_BYTES,
                    )
                    .await
                    .map(Some)
                } else {
                    Ok(None)
                }
            },
        );
        let head_ref = head_ref?;
        let head_object = head_object?;
        let configured = configured?;
        let directories = directories?;

        let branch_name = if head_ref.exit_code == 0 {
            head_ref.stdout.trim().to_owned()
        } else {
            String::new()
        };
        let head = if head_object.exit_code == 0 {
            head_object.stdout.trim().to_owned()
        } else {
            String::new()
        };
        let branch = if branch_name.is_empty() {
            RepositoryBranchState::detached()
        } else if head.is_empty() {
            RepositoryBranchState::unborn(branch_name)
                .unwrap_or_else(|_| RepositoryBranchState::detached())
        } else {
            RepositoryBranchState::attached(branch_name)
                .unwrap_or_else(|_| RepositoryBranchState::detached())
        };

        let remotes = if configured.exit_code == 0 {
            parse_configured_remotes(&configured.stdout)
                .iter()
                .filter_map(project_remote)
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let default_remote = default_remote_name(&remotes);
        let snapshot = RepositorySnapshot::new(branch, default_remote, remotes)
            .map_err(|_| GitReadError::Unavailable)?;
        // A Git too old for `--path-format` fails the read: the root then
        // counts as the main checkout.
        let linked_worktree = directories
            .filter(|output| output.exit_code == 0)
            .and_then(|output| linked_worktree(&output.stdout, &head));
        Ok(RepositoryObservation {
            repository: ProjectRepository::Repository(snapshot),
            linked_worktree,
        })
    }

    /// Runs one bounded Git read against the stored project root.
    async fn run_git(
        &self,
        root: &RootPath,
        args: &[&str],
        max_stdout: usize,
    ) -> Result<GitOutput, GitReadError> {
        let mut command = Command::new(&self.git_executable);
        command
            .arg("-C")
            .arg(root.as_str())
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(|_| GitReadError::Unavailable)?;
        let mut stdout = child.stdout.take().ok_or(GitReadError::Unavailable)?;
        let mut stderr = child.stderr.take().ok_or(GitReadError::Unavailable)?;
        let mut stdout_bytes = Vec::new();
        let mut stderr_bytes = Vec::new();
        let (stdout_read, stderr_read, status) = tokio::join!(
            read_bounded(&mut stdout, max_stdout, &mut stdout_bytes),
            read_bounded(&mut stderr, MAX_STDERR_BYTES, &mut stderr_bytes),
            child.wait(),
        );
        stdout_read.map_err(|_| GitReadError::Unavailable)?;
        stderr_read.map_err(|_| GitReadError::Unavailable)?;
        let status = status.map_err(|_| GitReadError::Unavailable)?;
        Ok(GitOutput {
            exit_code: status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&stdout_bytes).into_owned(),
        })
    }
}

/// One bounded Git invocation result.
struct GitOutput {
    exit_code: i32,
    stdout: String,
}

/// Reads at most `limit` bytes so a hostile repository cannot force an
/// unbounded allocation into the service.
async fn read_bounded<R>(reader: &mut R, limit: usize, output: &mut Vec<u8>) -> std::io::Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
{
    reader
        .take(u64::try_from(limit).unwrap_or(u64::MAX))
        .read_to_end(output)
        .await?;
    Ok(())
}

/// Classifies a durable catalog read failure without exposing database detail.
fn classify_catalog_error(_error: RepositoryError) -> ProjectRepositoryServiceError {
    ProjectRepositoryServiceError::CatalogUnavailable
}

#[cfg(test)]
mod tests {
    use super::*;
    use artisan_protocol::RepositoryHost;

    #[test]
    fn configured_remotes_follow_git_order_and_ignore_unrelated_keys() {
        let remotes = parse_configured_remotes(
            "remote.origin.url git@github.com:artisanstreet/editor.git\nremote.upstream.url https://gitlab.com/owner/repo.git\nuser.name Sander\nremote.origin.pushurl ignored\n",
        );
        assert_eq!(
            remotes,
            vec![
                ConfiguredRemote {
                    name: "origin".to_owned(),
                    url: "git@github.com:artisanstreet/editor.git".to_owned(),
                },
                ConfiguredRemote {
                    name: "upstream".to_owned(),
                    url: "https://gitlab.com/owner/repo.git".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn configured_remotes_drop_empty_and_duplicate_names() {
        let remotes = parse_configured_remotes(
            "remote..url https://example.test/repo.git\nremote.origin.url \nremote.origin.url https://github.com/owner/repo.git\nremote.origin.url https://github.com/owner/other.git\n",
        );
        assert_eq!(remotes.len(), 1);
        assert_eq!(remotes[0].url, "https://github.com/owner/repo.git");
    }

    #[test]
    fn raw_remote_syntaxes_project_to_host_and_web_url() {
        let cases = [
            (
                "https://github.com/owner/repository.git",
                RepositoryHost::GitHub,
                Some("https://github.com/owner/repository"),
            ),
            (
                "git@github.com:owner/repository.git",
                RepositoryHost::GitHub,
                Some("https://github.com/owner/repository"),
            ),
            (
                "ssh://git@gitlab.com/owner/repository.git",
                RepositoryHost::GitLab,
                Some("https://gitlab.com/owner/repository"),
            ),
            ("/srv/git/repository.git", RepositoryHost::Unknown, None),
        ];

        for (url, expected_host, expected_web_url) in cases {
            let remote = project_remote(&ConfiguredRemote {
                name: "origin".to_owned(),
                url: url.to_owned(),
            })
            .expect("remote projects");
            assert_eq!(remote.host(), expected_host, "url: {url}");
            assert_eq!(remote.web_url(), expected_web_url, "url: {url}");
        }
    }

    #[test]
    fn default_remote_prefers_origin_then_gits_order() {
        let remote = |name: &str| {
            RepositoryRemote::new(
                RepositoryHost::GitHub,
                name,
                "https://github.com/owner/repository.git",
                None,
            )
            .expect("remote")
        };

        assert_eq!(
            default_remote_name(&[remote("upstream"), remote("origin")]),
            Some("origin".to_owned())
        );
        assert_eq!(
            default_remote_name(&[remote("upstream"), remote("fork")]),
            Some("upstream".to_owned())
        );
        assert_eq!(default_remote_name(&[]), None);
    }

    #[test]
    fn project_host_maps_every_policy_vocabulary_entry() {
        for host in [
            PolicyRepositoryHost::Azure,
            PolicyRepositoryHost::Bitbucket,
            PolicyRepositoryHost::Codeberg,
            PolicyRepositoryHost::Gitea,
            PolicyRepositoryHost::Github,
            PolicyRepositoryHost::Gitlab,
            PolicyRepositoryHost::Other,
            PolicyRepositoryHost::Sourcehut,
            PolicyRepositoryHost::Unknown,
        ] {
            assert_eq!(project_host(host).as_str(), host.as_str());
        }
    }
}
