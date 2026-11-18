use crate::Turn;
use artisan_api::{EngineId, Permissions, ThreadId};
use forge_isolation::{Ruleset, RulesetError, Support};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const TEMPORARY_FOLDER_VARIABLES: [&str; 3] = ["TMPDIR", "TMP", "TEMP"];
const RUN_TEMPORARY_PREFIX: &str = "artisan-run-";

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
    let codex_home = turn
        .launch
        .env
        .iter()
        .filter(|(key, _)| key == "CODEX_HOME")
        .map(|(_, value)| PathBuf::from(value));
    [turn.cwd.clone(), turn.home.clone()]
        .into_iter()
        .chain(codex_home)
        .chain(shared_git_folder(&turn.cwd))
        .collect()
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
