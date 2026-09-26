//! The subtitle beneath a thread's title in the Editor's sidebar: the
//! repository a project publishes to, the branch of a linked worktree, or
//! the project's name; and the Git observation it derives from.

#![forbid(unsafe_code)]

use std::error::Error;
use std::path::Path;
use std::process::Command;

use artisan_backend::conversation_commit_notifier::ConversationCommitNotifier;
use artisan_backend::project_repository_service::{
    LinkedWorktree, ProjectRepositoryService, RepositoryObservation, linked_worktree,
};
use artisan_backend::project_subtitle_policy::project_subtitle;
use artisan_backend::project_subtitles::ProjectSubtitles;
use artisan_database::{Repository, SqliteConfig, connect};
use artisan_domain::{DisplayName, ProjectId, ProjectSummary, RootPath, UnixMillis};
use artisan_protocol::{
    ProjectRepository, RepositoryBranchState, RepositoryHost, RepositoryRemote, RepositorySnapshot,
};

fn name() -> DisplayName {
    DisplayName::parse("editor-folder").expect("display name")
}

fn repository(branch: RepositoryBranchState, urls: &[(&str, &str)]) -> ProjectRepository {
    let remotes = urls
        .iter()
        .map(|(name, url)| {
            RepositoryRemote::new(RepositoryHost::Other, *name, *url, None).expect("remote")
        })
        .collect::<Vec<_>>();
    let default_remote = remotes
        .iter()
        .find(|remote| remote.name() == "origin")
        .or_else(|| remotes.first())
        .map(|remote| remote.name().to_owned());
    ProjectRepository::Repository(
        RepositorySnapshot::new(branch, default_remote, remotes).expect("snapshot"),
    )
}

fn main_checkout(url: &str) -> RepositoryObservation {
    RepositoryObservation {
        repository: repository(
            RepositoryBranchState::attached("main").expect("branch"),
            &[("origin", url)],
        ),
        linked_worktree: None,
    }
}

fn subtitle(observation: Option<&RepositoryObservation>) -> String {
    project_subtitle(&name(), observation).as_str().to_owned()
}

#[test]
fn remotes_name_their_repository_without_the_host() {
    let cases = [
        ("https://github.com/owner/repo.git", "owner/repo"),
        ("https://github.com/owner/repo", "owner/repo"),
        ("git@github.com:owner/repo.git", "owner/repo"),
        ("ssh://git@github.com/owner/repo.git", "owner/repo"),
        (
            "ssh://git@gitlab.example.com:2222/group/sub/repo.git",
            "group/sub/repo",
        ),
        (
            "https://gitlab.com/group/subgroup/repo.git",
            "group/subgroup/repo",
        ),
        ("https://git.example.org/team/tool.git", "team/tool"),
        ("git@git.internal.example:platform/api.git", "platform/api"),
        (
            "git@ssh.dev.azure.com:v3/org/project/repo",
            "org/project/repo",
        ),
    ];
    for (url, expected) in cases {
        assert_eq!(subtitle(Some(&main_checkout(url))), expected, "url: {url}");
    }
}

#[test]
fn the_default_remote_names_the_repository() {
    let observation = RepositoryObservation {
        repository: repository(
            RepositoryBranchState::attached("main").expect("branch"),
            &[
                ("upstream", "https://github.com/upstream/repo.git"),
                ("origin", "git@github.com:fork/repo.git"),
            ],
        ),
        linked_worktree: None,
    };
    assert_eq!(subtitle(Some(&observation)), "fork/repo");
}

#[test]
fn projects_without_a_resolvable_remote_show_their_name() {
    let no_remotes = RepositoryObservation {
        repository: repository(
            RepositoryBranchState::attached("main").expect("branch"),
            &[],
        ),
        linked_worktree: None,
    };
    let not_repository = RepositoryObservation {
        repository: ProjectRepository::NotRepository,
        linked_worktree: None,
    };
    for observation in [
        Some(no_remotes),
        Some(not_repository),
        Some(main_checkout("/srv/git/repo.git")),
        Some(main_checkout("file:///srv/git/repo.git")),
        None,
    ] {
        assert_eq!(subtitle(observation.as_ref()), "editor-folder");
    }
}

#[test]
fn linked_worktrees_append_their_branch_or_detached_head() {
    let worktree =
        |branch: RepositoryBranchState, short_head: Option<&str>| RepositoryObservation {
            repository: repository(branch, &[("origin", "git@github.com:owner/repo.git")]),
            linked_worktree: Some(LinkedWorktree {
                directory_name: "repo-review".to_owned(),
                short_head: short_head.map(str::to_owned),
            }),
        };
    assert_eq!(
        subtitle(Some(&worktree(
            RepositoryBranchState::attached("sidebar-recents").expect("branch"),
            Some("abc1234"),
        ))),
        "owner/repo · sidebar-recents"
    );
    assert_eq!(
        subtitle(Some(&worktree(
            RepositoryBranchState::unborn("fresh").expect("branch"),
            None,
        ))),
        "owner/repo · fresh"
    );
    assert_eq!(
        subtitle(Some(&worktree(
            RepositoryBranchState::detached(),
            Some("abc1234")
        ))),
        "owner/repo · abc1234"
    );
    assert_eq!(
        subtitle(Some(&worktree(RepositoryBranchState::detached(), None))),
        "owner/repo · repo-review"
    );
}

#[test]
fn worktree_directories_distinguish_linked_from_main_checkouts() {
    assert_eq!(
        linked_worktree("/src/repo/.git\n/src/repo/.git\n/src/repo\n", "0123456789"),
        None
    );
    assert_eq!(
        linked_worktree(
            "/src/repo/.git/worktrees/review\n/src/repo/.git\n/src/review\n",
            "0123456789abcdef",
        ),
        Some(LinkedWorktree {
            directory_name: "review".to_owned(),
            short_head: Some("0123456".to_owned()),
        })
    );
    assert_eq!(linked_worktree("", "0123456789"), None);
}

fn git(directory: &Path, args: &[&str]) -> Result<(), Box<dyn Error>> {
    let status = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
        .args(args)
        .output()?;
    if !status.status.success() {
        return Err(format!(
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&status.stderr)
        )
        .into());
    }
    Ok(())
}

/// A real repository and a worktree of it: the main checkout names the
/// repository, the worktree adds its branch, and the cached subtitles pick
/// them up from a background observation.
#[tokio::test]
async fn observed_worktrees_resolve_through_the_subtitle_cache() -> Result<(), Box<dyn Error>> {
    let base = std::env::temp_dir().join(format!(
        "artisan-subtitles-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    let _cleanup = std::fs::remove_dir_all(&base);
    let main = base.join("repo");
    let review = base.join("repo-review");
    std::fs::create_dir_all(&main)?;
    git(&main, &["init", "-q", "-b", "main"])?;
    git(
        &main,
        &["remote", "add", "origin", "git@github.com:owner/repo.git"],
    )?;
    git(&main, &["commit", "-q", "--allow-empty", "-m", "initial"])?;
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "review",
            review.to_str().ok_or("path")?,
        ],
    )?;

    let database = connect(SqliteConfig::in_memory().sqlx_logging(false)).await?;
    let service = ProjectRepositoryService::new(Repository::new(database));
    let root = |path: &Path| RootPath::parse(path.to_string_lossy().into_owned());
    let main_observation = service.observe_root(&root(&main)?).await?;
    assert_eq!(main_observation.linked_worktree, None);
    let review_observation = service.observe_root(&root(&review)?).await?;
    assert_eq!(
        review_observation
            .linked_worktree
            .as_ref()
            .map(|worktree| worktree.directory_name.as_str()),
        Some("repo-review")
    );

    let subtitles = ProjectSubtitles::new(service);
    let notifier = ConversationCommitNotifier::new();
    let project = |id: &str, path: &Path| -> Result<ProjectSummary, Box<dyn Error>> {
        Ok(ProjectSummary {
            project_id: ProjectId::parse(id)?,
            display_name: DisplayName::parse(id)?,
            root_path: root(path)?,
            attached_at: UnixMillis::from_millis(1),
        })
    };
    let projects = [project("main", &main)?, project("review", &review)?];
    let first = subtitles.subtitles(&projects, &notifier);
    assert_eq!(first[&ProjectId::parse("main")?].as_str(), "main");
    let generation = subtitles.generation();
    for _ in 0..200 {
        if subtitles.generation() != generation {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let resolved = subtitles.subtitles(&projects, &notifier);
    assert_eq!(resolved[&ProjectId::parse("main")?].as_str(), "owner/repo");
    assert_eq!(
        resolved[&ProjectId::parse("review")?].as_str(),
        "owner/repo · review"
    );
    let _cleanup = std::fs::remove_dir_all(&base);
    Ok(())
}
