use crate::Hub;
use crate::error::ErrorClass;
use artisan_api::{CreateProject, ProjectId, Thread, ThreadId, Timestamp};
use forge_store::{CreateProjectError, ReadProjectError};
use std::path::{Path, PathBuf};

const FOLDER_WORDS: usize = 4;
const FOLDER_WORDS_BYTES: usize = 48;
const LEGACY_KEY: usize = 12;

#[derive(Debug, thiserror::Error)]
pub enum EnsureScratchProjectError {
    #[error("create the scratch folder {}", path.display())]
    CreateFolder {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("run the scratch folder task")]
    Task(#[source] tokio::task::JoinError),
    #[error("register the scratch project")]
    CreateProject(#[source] CreateProjectError),
}

#[derive(Debug, thiserror::Error)]
pub enum ThreadFolderError {
    #[error("run the scratch folder task of thread {thread}")]
    Task {
        thread: ThreadId,
        #[source]
        source: tokio::task::JoinError,
    },
    #[error("find or create the scratch folder of thread {thread}")]
    Scratch {
        thread: ThreadId,
        #[source]
        source: ScratchFolderError,
    },
    #[error("read project {project} of thread {thread}")]
    ReadProject {
        thread: ThreadId,
        project: ProjectId,
        #[source]
        source: ReadProjectError,
    },
    #[error("thread {thread} belongs to project {project}, which Forge doesn't have")]
    NoProject {
        thread: ThreadId,
        project: ProjectId,
    },
}

impl ThreadFolderError {
    #[must_use]
    pub(crate) const fn class(&self) -> ErrorClass {
        match self {
            Self::Task { .. }
            | Self::Scratch { .. }
            | Self::ReadProject { .. }
            | Self::NoProject { .. } => ErrorClass::Internal,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ScratchFolderError {
    #[error("list the scratch folders in {}", path.display())]
    List {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("create {}", path.display())]
    Create {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum RemoveScratchFolderError {
    #[error("list the scratch folders in {}", path.display())]
    List {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("remove {}", path.display())]
    Remove {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl Hub {
    pub(crate) fn scratch_root(&self) -> PathBuf {
        self.data_dir().join("scratch")
    }

    pub(crate) fn sandbox_tool_caches(&self, project: &ProjectId) -> PathBuf {
        self.data_dir()
            .join("sandbox")
            .join(project.as_str())
            .join("caches")
    }

    pub async fn ensure_scratch_project(&self) -> Result<(), EnsureScratchProjectError> {
        let root = self.scratch_root();
        if inside_work_tree(self.data_dir()) {
            tracing::info!(data = %self.data_dir().display(), "no scratch project inside a git work tree");
            return Ok(());
        }
        let created = root.clone();
        tokio::task::spawn_blocking(move || std::fs::create_dir_all(&created))
            .await
            .map_err(EnsureScratchProjectError::Task)?
            .map_err(|source| EnsureScratchProjectError::CreateFolder {
                path: root.clone(),
                source,
            })?;
        self.store()
            .create_project(CreateProject {
                id: ProjectId::scratch(),
                path: root.display().to_string(),
                name: Some("No project".to_owned()),
            })
            .await
            .map_err(EnsureScratchProjectError::CreateProject)?;
        Ok(())
    }

    pub(crate) async fn thread_folder(
        &self,
        thread: &Thread,
    ) -> Result<PathBuf, ThreadFolderError> {
        if thread.project_id.is_scratch() {
            let (root, scratch) = (self.scratch_root(), thread.clone());
            return tokio::task::spawn_blocking(move || scratch_folder(&root, &scratch))
                .await
                .map_err(|source| ThreadFolderError::Task {
                    thread: thread.id.clone(),
                    source,
                })?
                .map_err(|source| ThreadFolderError::Scratch {
                    thread: thread.id.clone(),
                    source,
                });
        }
        let project = self
            .store()
            .project(&thread.project_id)
            .await
            .map_err(|source| ThreadFolderError::ReadProject {
                thread: thread.id.clone(),
                project: thread.project_id.clone(),
                source,
            })?
            .ok_or_else(|| ThreadFolderError::NoProject {
                thread: thread.id.clone(),
                project: thread.project_id.clone(),
            })?;
        Ok(PathBuf::from(project.path))
    }
}

fn inside_work_tree(path: &Path) -> bool {
    path.ancestors().any(|dir| dir.join(".git").exists())
}

enum FolderKey {
    Canonical { key: String, legacy: String },
    Hashed(String),
}

impl FolderKey {
    fn of(thread: &ThreadId) -> Self {
        let id = thread.as_str();
        match uuid::Uuid::try_parse(id) {
            Ok(parsed) if parsed.hyphenated().to_string() == id => {
                let key = parsed.simple().to_string();
                let legacy = key.split_at(key.len() - LEGACY_KEY).1.to_owned();
                Self::Canonical { key, legacy }
            }
            Ok(_) | Err(_) => {
                use sha2::Digest as _;
                let digest = hex::encode(sha2::Sha256::digest(id.as_bytes()));
                Self::Hashed(format!("x{}", digest.split_at(32).0))
            }
        }
    }

    fn current(&self) -> &str {
        match self {
            Self::Canonical { key, .. } | Self::Hashed(key) => key,
        }
    }
}

fn existing_scratch_folder(root: &Path, thread: &Thread) -> std::io::Result<Option<PathBuf>> {
    let key = FolderKey::of(&thread.id);
    let current = format!("-{}", key.current());
    let legacy = match &key {
        FolderKey::Canonical { legacy, .. } => {
            Some((date(thread.created_at), format!("-{legacy}")))
        }
        FolderKey::Hashed(_) => None,
    };
    let mut found_legacy = None;
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !entry.file_type()?.is_dir() {
            continue;
        }
        if name.ends_with(&current) {
            return Ok(Some(entry.path()));
        }
        if let Some((date, suffix)) = &legacy
            && name.starts_with(date.as_str())
            && name.ends_with(suffix.as_str())
        {
            found_legacy = Some(entry.path());
        }
    }
    Ok(found_legacy)
}

pub(crate) fn remove_scratch_folder(
    root: &Path,
    thread: &Thread,
) -> Result<(), RemoveScratchFolderError> {
    if !root.is_dir() {
        return Ok(());
    }
    let found =
        existing_scratch_folder(root, thread).map_err(|source| RemoveScratchFolderError::List {
            path: root.to_path_buf(),
            source,
        })?;
    let Some(folder) = found else {
        return Ok(());
    };
    std::fs::remove_dir_all(&folder).map_err(|source| RemoveScratchFolderError::Remove {
        path: folder,
        source,
    })
}

fn scratch_folder(root: &Path, thread: &Thread) -> Result<PathBuf, ScratchFolderError> {
    let key = FolderKey::of(&thread.id);
    std::fs::create_dir_all(root).map_err(|source| ScratchFolderError::Create {
        path: root.to_path_buf(),
        source,
    })?;
    let found =
        existing_scratch_folder(root, thread).map_err(|source| ScratchFolderError::List {
            path: root.to_path_buf(),
            source,
        })?;
    if let Some(folder) = found {
        return Ok(folder);
    }
    let words = thread
        .title
        .as_deref()
        .map(folder_words)
        .unwrap_or_default();
    let name = [date(thread.created_at), words, key.current().to_owned()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let folder = root.join(name);
    std::fs::create_dir_all(&folder).map_err(|source| ScratchFolderError::Create {
        path: folder.clone(),
        source,
    })?;
    Ok(folder)
}

fn folder_words(text: &str) -> String {
    let mut words = text
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .take(FOLDER_WORDS)
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>()
        .join("-");
    words.truncate(FOLDER_WORDS_BYTES);
    words.trim_end_matches('-').to_owned()
}

pub(crate) fn date(at: Timestamp) -> String {
    let days = at.div_euclid(86_400_000);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}
