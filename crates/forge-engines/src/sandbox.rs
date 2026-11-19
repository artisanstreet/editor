use crate::Turn;
use artisan_api::{EngineId, Permissions, ThreadId};
use forge_isolation::{Ruleset, RulesetError, Support};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const TEMPORARY_FOLDER_VARIABLES: [&str; 3] = ["TMPDIR", "TMP", "TEMP"];
const RUN_TEMPORARY_PREFIX: &str = "artisan-run-";
const SHARED_TEMPORARY: [&str; 2] = ["/tmp", "/dev/shm"];
const SHARED_CACHES: [&str; 2] = [".npm", ".local/share/pnpm"];
const GIT_POINTER_BYTES: u64 = 64 * 1024;
const TOOL_CACHES: [(&str, &str); 6] = [
    (".cargo", "registry"),
    (".cargo", "git"),
    (".rustup", "tmp"),
    (".bun", "install/cache"),
    (".gradle", "caches"),
    (".m2", "repository"),
];

#[derive(Debug, Clone, PartialEq, Eq)]
enum Writable {
    Existing(PathBuf),
    InToolHome { tool_home: PathBuf, cache: PathBuf },
}

#[derive(Clone, Debug)]
pub enum Sandbox {
    Open,
    Confined(Arc<Confined>),
}

#[derive(Debug)]
pub struct Confined {
    temporary: tempfile::TempDir,
    ruleset: Ruleset,
}

#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error("create the temporary folder of {engine} thread {thread}'s sandbox")]
    Temporary {
        engine: EngineId,
        thread: ThreadId,
        #[source]
        source: std::io::Error,
    },
    #[error("confine {engine} thread {thread} to its writable folders")]
    Ruleset {
        engine: EngineId,
        thread: ThreadId,
        #[source]
        source: RulesetError,
    },
}

impl Sandbox {
    pub(crate) fn for_turn(turn: &Turn) -> Result<Self, SandboxError> {
        match (turn.config.permissions, forge_isolation::support()) {
            (Permissions::Full, _) | (Permissions::Auto, Support::Unavailable(_)) => Ok(Self::Open),
            (Permissions::Auto, Support::Enforced(landlock)) => {
                let temporary = tempfile::Builder::new()
                    .prefix(RUN_TEMPORARY_PREFIX)
                    .tempdir()
                    .map_err(|source| SandboxError::Temporary {
                        engine: turn.engine,
                        thread: turn.thread_id.clone(),
                        source,
                    })?;
                let mut roots = writable_roots(turn);
                roots.push(temporary.path().to_path_buf());
                let ruleset = landlock
                    .writes_only_to(&roots, &reachable_sockets(turn))
                    .map_err(|source| SandboxError::Ruleset {
                        engine: turn.engine,
                        thread: turn.thread_id.clone(),
                        source,
                    })?;
                Ok(Self::Confined(Arc::new(Confined { temporary, ruleset })))
            }
        }
    }

    pub(crate) fn ruleset(&self) -> Option<&Ruleset> {
        match self {
            Self::Open => None,
            Self::Confined(confined) => Some(&confined.ruleset),
        }
    }

    pub(crate) const fn confined(&self) -> bool {
        matches!(self, Self::Confined(_))
    }

    pub(crate) fn prepare(&self, command: &mut tokio::process::Command) {
        if let Self::Confined(confined) = self {
            for variable in TEMPORARY_FOLDER_VARIABLES {
                command.env(variable, confined.temporary.path());
            }
        }
    }
}

pub(crate) const fn forge_confines(permissions: Permissions, support: Support) -> bool {
    matches!(
        (permissions, support),
        (Permissions::Auto, Support::Enforced(_))
    )
}

fn launch_variable(turn: &Turn, name: &str) -> Option<PathBuf> {
    turn.launch
        .env
        .iter()
        .rev()
        .find(|(key, _)| key == name)
        .map(|(_, value)| PathBuf::from(value))
        .or_else(|| std::env::var_os(name).map(PathBuf::from))
        .filter(|path| path.is_absolute())
}

fn reachable_sockets(turn: &Turn) -> Vec<PathBuf> {
    launch_variable(turn, "SSH_AUTH_SOCK").into_iter().collect()
}

fn writable_roots(turn: &Turn) -> Vec<PathBuf> {
    let variable = |name: &str| launch_variable(turn, name);
    let codex_home = turn
        .launch
        .env
        .iter()
        .filter(|(key, _)| key == "CODEX_HOME")
        .map(|(_, value)| PathBuf::from(value));
    let caches = tool_caches(
        variable("HOME").as_deref(),
        variable("XDG_CACHE_HOME"),
        variable("TMPDIR"),
    )
    .into_iter()
    .filter_map(ready);
    [turn.cwd.clone(), turn.home.clone()]
        .into_iter()
        .chain(codex_home)
        .chain(shared_git_folder(&turn.cwd))
        .chain(caches)
        .collect()
}

fn tool_caches(
    home: Option<&Path>,
    cache_home: Option<PathBuf>,
    temporary: Option<PathBuf>,
) -> Vec<Writable> {
    let shared = SHARED_TEMPORARY
        .into_iter()
        .map(PathBuf::from)
        .chain(temporary)
        .chain(cache_home.or_else(|| home.map(|home| home.join(".cache"))))
        .chain(
            home.into_iter()
                .flat_map(|home| SHARED_CACHES.map(|cache| home.join(cache))),
        )
        .map(Writable::Existing);
    let tools = home.into_iter().flat_map(|home| {
        TOOL_CACHES.map(|(tool, cache)| Writable::InToolHome {
            tool_home: home.join(tool),
            cache: home.join(tool).join(cache),
        })
    });
    shared.chain(tools).collect()
}

fn ready(writable: Writable) -> Option<PathBuf> {
    match writable {
        Writable::Existing(path) => Some(path),
        Writable::InToolHome { tool_home, cache } if tool_home.is_dir() => {
            match std::fs::create_dir_all(&cache) {
                Ok(()) => Some(cache),
                Err(error) => {
                    tracing::warn!(path = %cache.display(), %error, "the sandbox can't create a tool cache; the engine can't write it");
                    None
                }
            }
        }
        Writable::InToolHome { .. } => None,
    }
}

fn shared_git_folder(project: &Path) -> Option<PathBuf> {
    let pointer_file = project.join(".git");
    if !pointer_file.is_file() {
        return None;
    }
    match linked_git_folder(project, &pointer_file) {
        Ok(folder) => Some(folder),
        Err(refusal) => {
            let error: &dyn std::error::Error = &refusal;
            tracing::warn!(project = %project.display(), error, "the sandbox doesn't let git write outside the project");
            None
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum GitLinkRefused {
    #[error("read the git pointer {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{} isn't a `gitdir:` pointer", path.display())]
    NotAPointer { path: PathBuf },
    #[error("resolve {}", path.display())]
    Resolve {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("git folder {} doesn't list {} as its checkout", git_folder.display(), project.display())]
    NotLinked {
        git_folder: PathBuf,
        project: PathBuf,
    },
    #[error("{} isn't a git repository", folder.display())]
    NotARepository { folder: PathBuf },
    #[error("worktree folder {} names {} as its shared folder, not {}", git_folder.display(), named.display(), common.display())]
    ForeignCommon {
        git_folder: PathBuf,
        named: PathBuf,
        common: PathBuf,
    },
    #[error("shared git folder {} holds the project {}", common.display(), project.display())]
    HoldsProject { common: PathBuf, project: PathBuf },
}

fn linked_git_folder(project: &Path, pointer_file: &Path) -> Result<PathBuf, GitLinkRefused> {
    let pointer = read_trimmed(pointer_file)?;
    let target = pointer
        .strip_prefix("gitdir:")
        .ok_or_else(|| GitLinkRefused::NotAPointer {
            path: pointer_file.to_path_buf(),
        })?
        .trim();
    let project = canonical(project)?;
    let checkout = project.join(".git");
    let git_folder = canonical(&project.join(target))?;
    let common = if lists_worktree(&git_folder, &checkout)? {
        worktree_common(&git_folder)?
    } else if lists_submodule(&git_folder, &project)? {
        git_folder
    } else {
        return Err(GitLinkRefused::NotLinked {
            git_folder,
            project,
        });
    };
    if !is_repository(&common) {
        return Err(GitLinkRefused::NotARepository { folder: common });
    }
    if project.starts_with(&common) {
        return Err(GitLinkRefused::HoldsProject { common, project });
    }
    Ok(common)
}

fn lists_worktree(git_folder: &Path, checkout: &Path) -> Result<bool, GitLinkRefused> {
    let backlink = git_folder.join("gitdir");
    if !backlink.is_file() {
        return Ok(false);
    }
    let listed = read_trimmed(&backlink)?;
    Ok(canonical(&git_folder.join(listed))? == checkout)
}

fn worktree_common(git_folder: &Path) -> Result<PathBuf, GitLinkRefused> {
    let not_a_worktree = || GitLinkRefused::NotARepository {
        folder: git_folder.to_path_buf(),
    };
    let worktrees = git_folder.parent().ok_or_else(not_a_worktree)?;
    if worktrees.file_name() != Some(std::ffi::OsStr::new("worktrees")) {
        return Err(not_a_worktree());
    }
    let common = worktrees.parent().ok_or_else(not_a_worktree)?.to_path_buf();
    let named = read_trimmed(&git_folder.join("commondir"))?;
    let named = canonical(&git_folder.join(named))?;
    if named != common {
        return Err(GitLinkRefused::ForeignCommon {
            git_folder: git_folder.to_path_buf(),
            named,
            common,
        });
    }
    Ok(common)
}

fn lists_submodule(git_folder: &Path, project: &Path) -> Result<bool, GitLinkRefused> {
    let config = git_folder.join("config");
    if !config.is_file() {
        return Ok(false);
    }
    let config = read_trimmed(&config)?;
    match core_worktree(&config) {
        Some(listed) => Ok(canonical(&git_folder.join(listed))? == project),
        None => Ok(false),
    }
}

fn core_worktree(config: &str) -> Option<&str> {
    let mut in_core = false;
    config.lines().map(str::trim).find_map(|line| {
        if line.starts_with('[') {
            in_core = line.eq_ignore_ascii_case("[core]");
            return None;
        }
        let (key, value) = line.split_once('=')?;
        (in_core && key.trim().eq_ignore_ascii_case("worktree")).then(|| value.trim())
    })
}

fn is_repository(folder: &Path) -> bool {
    folder.join("HEAD").is_file() && folder.join("objects").is_dir() && folder.join("refs").is_dir()
}

fn read_trimmed(path: &Path) -> Result<String, GitLinkRefused> {
    let read = |path: &Path| -> std::io::Result<String> {
        let mut text = String::new();
        std::io::Read::read_to_string(
            &mut std::io::Read::take(std::fs::File::open(path)?, GIT_POINTER_BYTES),
            &mut text,
        )?;
        Ok(text.trim().to_owned())
    };
    read(path).map_err(|source| GitLinkRefused::Read {
        path: path.to_path_buf(),
        source,
    })
}

fn canonical(path: &Path) -> Result<PathBuf, GitLinkRefused> {
    std::fs::canonicalize(path).map_err(|source| GitLinkRefused::Resolve {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
#[cfg(target_os = "linux")]
#[path = "../tests/sandbox/launch.rs"]
mod tests;
