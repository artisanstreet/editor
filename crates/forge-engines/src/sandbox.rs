use crate::Turn;
use artisan_api::{EngineId, Permissions, ThreadId};
use forge_isolation::{Ruleset, RulesetError, Support};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const TEMPORARY_FOLDER_VARIABLES: [&str; 3] = ["TMPDIR", "TMP", "TEMP"];
const RUN_TEMPORARY_PREFIX: &str = "artisan-run-";
const SHARED_TEMPORARY: [&str; 2] = ["/tmp", "/dev/shm"];
const SHARED_CACHES: [&str; 2] = [".npm", ".local/share/pnpm"];
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
                let ruleset =
                    landlock
                        .writes_only_to(&roots)
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

fn writable_roots(turn: &Turn) -> Vec<PathBuf> {
    let variable = |name: &str| {
        turn.launch
            .env
            .iter()
            .rev()
            .find(|(key, _)| key == name)
            .map(|(_, value)| PathBuf::from(value))
            .or_else(|| std::env::var_os(name).map(PathBuf::from))
            .filter(|path| path.is_absolute())
    };
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
    let pointer = match std::fs::read_to_string(&pointer_file) {
        Ok(pointer) => pointer,
        Err(error) => {
            tracing::warn!(path = %pointer_file.display(), %error, "the sandbox can't read the project's git pointer; git can't write its shared folder");
            return None;
        }
    };
    let git_folder = project.join(pointer.strip_prefix("gitdir:")?.trim());
    let common = git_folder.join("commondir");
    match std::fs::read_to_string(&common) {
        Ok(relative) => Some(git_folder.join(relative.trim())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(git_folder),
        Err(error) => {
            tracing::warn!(path = %common.display(), %error, "the sandbox can't read the worktree's shared git folder; git writes only to the worktree's own");
            Some(git_folder)
        }
    }
}

#[cfg(test)]
#[cfg(target_os = "linux")]
#[path = "../tests/sandbox/launch.rs"]
mod tests;
