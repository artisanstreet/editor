use crate::Turn;
use artisan_api::{EngineId, Permissions, ThreadId};
use forge_isolation::{Ruleset, RulesetError, Support};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const TEMPORARY_FOLDER_VARIABLES: [&str; 3] = ["TMPDIR", "TMP", "TEMP"];
const RUN_TEMPORARY_PREFIX: &str = "artisan-run-";
const SHARED_TEMPORARY: [&str; 2] = ["/tmp", "/dev/shm"];
const GIT_POINTER_BYTES: u64 = 64 * 1024;
const TOOL_CACHES: [(&str, &str); 8] = [
    ("XDG_CACHE_HOME", "cache"),
    ("CARGO_HOME", "cargo"),
    ("npm_config_cache", "npm"),
    ("npm_config_store_dir", "pnpm-store"),
    ("pnpm_config_store_dir", "pnpm-store"),
    ("BUN_INSTALL_CACHE_DIR", "bun"),
    ("GRADLE_USER_HOME", "gradle"),
    ("GOMODCACHE", "go-mod"),
];
const MAVEN_REPOSITORY: &str = "maven";

#[derive(Clone, Debug)]
pub(crate) enum Sandbox {
    Open,
    Confined(Arc<Confined>),
}

#[cfg(target_os = "linux")]
pub(crate) mod guard;

#[cfg(not(target_os = "linux"))]
pub(crate) mod guard {
    use artisan_api::EngineId;
    use std::path::{Path, PathBuf};

    #[derive(Debug, thiserror::Error)]
    pub enum GuardError {
        #[error("write {engine}'s configuration {}", path.display())]
        Write {
            engine: EngineId,
            path: PathBuf,
            #[source]
            source: Box<artisan_paths::private::PrivateFileError>,
        },
        #[error("read {engine}'s configuration {}", path.display())]
        Read {
            engine: EngineId,
            path: PathBuf,
            #[source]
            source: std::io::Error,
        },
        #[error("remove {engine}'s configuration {}", path.display())]
        Remove {
            engine: EngineId,
            path: PathBuf,
            #[source]
            source: std::io::Error,
        },
    }

    #[derive(Debug)]
    pub(crate) struct Exposure;

    impl Exposure {
        #[expect(
            clippy::unused_self,
            reason = "the same signature as Linux, where an exposure can outlive its runs"
        )]
        pub(crate) const fn outlive_forge(&self) {}
    }

    #[expect(
        clippy::unnecessary_wraps,
        reason = "the same signature as Linux, where guarding the engine's configuration can fail"
    )]
    pub(crate) const fn expose(_engine: EngineId, _home: &Path) -> Result<Exposure, GuardError> {
        Ok(Exposure)
    }

    #[expect(
        clippy::unnecessary_wraps,
        reason = "the same signature as Linux, where guarding the engine's configuration can fail"
    )]
    pub(crate) const fn restore(_engine: EngineId, _home: &Path) -> Result<(), GuardError> {
        Ok(())
    }

    pub(crate) fn write(
        engine: EngineId,
        home: &Path,
        relative: &str,
        bytes: &[u8],
        _mode: u32,
    ) -> Result<(), GuardError> {
        let path = home.join(relative);
        let written = if std::fs::read(&path).is_ok_and(|current| current == bytes) {
            artisan_paths::private::restrict_file(&path)
        } else {
            artisan_paths::private::write_file(&path, bytes)
        };
        written.map_err(|source| GuardError::Write {
            engine,
            path,
            source: Box::new(source),
        })
    }

    pub(crate) fn read(
        engine: EngineId,
        home: &Path,
        relative: &str,
    ) -> Result<Option<Vec<u8>>, GuardError> {
        let path = home.join(relative);
        match std::fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(GuardError::Read {
                engine,
                path,
                source,
            }),
        }
    }

    pub(crate) fn remove(engine: EngineId, home: &Path, relative: &str) -> Result<(), GuardError> {
        let path = home.join(relative);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(GuardError::Remove {
                engine,
                path,
                source,
            }),
        }
    }
}

pub(crate) use guard::GuardError;

#[derive(Debug)]
pub(crate) struct Confined {
    temporary: tempfile::TempDir,
    ruleset: Ruleset,
    redirects: Vec<(String, OsString)>,
    exposure: guard::Exposure,
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
    #[error("create the tool caches {} of {engine} thread {thread}'s sandbox", path.display())]
    ToolCaches {
        engine: EngineId,
        thread: ThreadId,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("guard {engine}'s configuration from thread {thread}'s sandbox")]
    Guard {
        engine: EngineId,
        thread: ThreadId,
        #[source]
        source: GuardError,
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
    pub(crate) async fn for_turn(turn: &Turn) -> Result<Self, SandboxError> {
        let turn = turn.clone();
        tokio::task::spawn_blocking(move || Self::build(&turn))
            .await
            .expect("invariant: building a sandbox does not panic")
    }

    pub(crate) fn build(turn: &Turn) -> Result<Self, SandboxError> {
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
                let caches = turn.launch.tool_caches.as_deref();
                let folders = caches.into_iter().flat_map(|caches| {
                    cache_folders(caches).chain(std::iter::once(caches.to_path_buf()))
                });
                for folder in folders {
                    std::fs::create_dir_all(&folder).map_err(|source| {
                        SandboxError::ToolCaches {
                            engine: turn.engine,
                            thread: turn.thread_id.clone(),
                            path: folder.clone(),
                            source,
                        }
                    })?;
                }
                let redirects = caches.map_or_else(Vec::new, |caches| redirects(turn, caches));
                let roots = writable_roots(turn);
                let ruleset = landlock
                    .writes_only_to(&roots, temporary.path(), &reachable_sockets(turn))
                    .map_err(|source| SandboxError::Ruleset {
                        engine: turn.engine,
                        thread: turn.thread_id.clone(),
                        source,
                    })?;
                let exposure = guard::expose(turn.engine, &turn.home).map_err(|source| {
                    SandboxError::Guard {
                        engine: turn.engine,
                        thread: turn.thread_id.clone(),
                        source,
                    }
                })?;
                Ok(Self::Confined(Arc::new(Confined {
                    temporary,
                    ruleset,
                    redirects,
                    exposure,
                })))
            }
        }
    }

    pub(crate) fn ruleset(&self) -> Option<&Ruleset> {
        match self {
            Self::Open => None,
            Self::Confined(confined) => Some(&confined.ruleset),
        }
    }

    pub(crate) fn outlived(&self) {
        if let Self::Confined(confined) = self {
            confined.exposure.outlive_forge();
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
            command.envs(confined.redirects.iter().map(|(key, value)| (key, value)));
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
    launch_value(turn, name)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

fn reachable_sockets(turn: &Turn) -> Vec<PathBuf> {
    launch_variable(turn, "SSH_AUTH_SOCK").into_iter().collect()
}

fn writable_roots(turn: &Turn) -> Vec<PathBuf> {
    let codex_home = turn
        .launch
        .env
        .iter()
        .filter(|(key, _)| key == "CODEX_HOME")
        .map(|(_, value)| PathBuf::from(value));
    let temporary = SHARED_TEMPORARY
        .into_iter()
        .map(PathBuf::from)
        .chain(launch_variable(turn, "TMPDIR"));
    [turn.cwd.clone(), turn.home.clone()]
        .into_iter()
        .chain(codex_home)
        .chain(turn.launch.git_common.clone())
        .chain(temporary)
        .chain(turn.launch.tool_caches.clone())
        .collect()
}

fn redirects(turn: &Turn, caches: &Path) -> Vec<(String, OsString)> {
    let engine_owned = |variable: &str| turn.launch.env.iter().any(|(key, _)| key == variable);
    let maven = std::iter::once((
        "MAVEN_OPTS".to_owned(),
        maven_options(
            launch_value(turn, "MAVEN_OPTS"),
            &caches.join(MAVEN_REPOSITORY),
        ),
    ));
    TOOL_CACHES
        .into_iter()
        .filter(|(variable, _)| !engine_owned(variable))
        .map(|(variable, folder)| (variable.to_owned(), caches.join(folder).into_os_string()))
        .chain(maven)
        .collect()
}

fn cache_folders(caches: &Path) -> impl Iterator<Item = PathBuf> + '_ {
    TOOL_CACHES
        .into_iter()
        .map(|(_, folder)| folder)
        .chain(std::iter::once(MAVEN_REPOSITORY))
        .map(|folder| caches.join(folder))
}

fn maven_options(inherited: Option<OsString>, repository: &Path) -> OsString {
    let mut options = inherited.unwrap_or_default();
    if !options.is_empty() {
        options.push(" ");
    }
    options.push("-Dmaven.repo.local=");
    options.push(repository);
    options
}

fn launch_value(turn: &Turn, name: &str) -> Option<OsString> {
    turn.launch
        .env
        .iter()
        .rev()
        .find(|(key, _)| key == name)
        .map(|(_, value)| OsString::from(value))
        .or_else(|| std::env::var_os(name))
}

#[must_use]
pub fn shared_git_folder(project: &Path) -> Option<PathBuf> {
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
