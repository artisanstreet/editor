use artisan_api::{ItemId, ThreadId};
use forge_engines::UserInput;
use forge_store::{DataLock, Store, ThreadIdsError};
use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const ROOT: &str = "run-images";

static PARTIAL: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, thiserror::Error)]
pub(crate) enum ImageCopyError {
    #[error("copy the image {} to {} for thread {thread}", from.display(), to.display())]
    Copy {
        thread: ThreadId,
        from: PathBuf,
        to: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("the stored image {} has no file name", path.display())]
    Unnamed { path: PathBuf },
    #[error("run the image copy task of thread {thread}")]
    Task {
        thread: ThreadId,
        #[source]
        source: tokio::task::JoinError,
    },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum SweepError {
    #[error("list the image folders in {}", path.display())]
    List {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error(transparent)]
    Threads(#[from] ThreadIdsError),
    #[error(transparent)]
    Remove(#[from] RemoveImagesError),
}

#[derive(Debug, thiserror::Error)]
#[error("remove the image folder {}", path.display())]
pub(crate) struct RemoveImagesError {
    path: PathBuf,
    #[source]
    source: io::Error,
}

#[derive(Debug)]
pub(crate) struct Message {
    id: ItemId,
    text: String,
    stored: Vec<PathBuf>,
}

impl Message {
    pub(crate) const fn new(id: ItemId, text: String, stored: Vec<PathBuf>) -> Self {
        Self { id, text, stored }
    }

    pub(crate) const fn text_only(id: ItemId, text: String) -> Self {
        Self::new(id, text, Vec::new())
    }

    pub(crate) const fn id(&self) -> &ItemId {
        &self.id
    }

    pub(crate) fn unattached(self) -> UserInput {
        let note = match self.stored.len() {
            1 => "[The image sent with this message couldn't be attached.]".to_owned(),
            count => format!("[The {count} images sent with this message couldn't be attached.]"),
        };
        let text = if self.text.is_empty() {
            note
        } else {
            format!("{}\n\n{note}", self.text)
        };
        UserInput {
            id: self.id,
            text,
            images: Vec::new(),
        }
    }
}

pub(crate) fn thread_folder(data: &Path, thread: &ThreadId) -> PathBuf {
    data.join(ROOT).join(crate::scratch::folder_key(thread))
}

fn remove_folder(path: PathBuf) -> Result<(), RemoveImagesError> {
    match std::fs::remove_dir_all(&path) {
        Err(source) if source.kind() != io::ErrorKind::NotFound => {
            Err(RemoveImagesError { path, source })
        }
        Ok(()) | Err(_) => Ok(()),
    }
}

pub(crate) fn remove_thread_folder(
    data: &Path,
    thread: &ThreadId,
) -> Result<(), RemoveImagesError> {
    remove_folder(thread_folder(data, thread))
}

pub(crate) async fn sweep(
    data: &Path,
    store: &Store,
    _exclusive: &DataLock,
) -> Result<usize, SweepError> {
    let root = data.join(ROOT);
    let listed = match std::fs::read_dir(&root) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        listed => listed
            .and_then(|entries| {
                entries
                    .map(|entry| entry.map(|entry| entry.file_name()))
                    .collect::<io::Result<Vec<_>>>()
            })
            .map_err(|source| SweepError::List {
                path: root.clone(),
                source,
            })?,
    };
    let live: HashSet<String> = store
        .thread_ids()
        .await?
        .iter()
        .map(crate::scratch::folder_key)
        .collect();
    let orphans: Vec<_> = listed
        .into_iter()
        .filter(|name| !live.contains(name.to_string_lossy().as_ref()))
        .collect();
    let removed = orphans.len();
    for name in orphans {
        remove_folder(root.join(name))?;
    }
    Ok(removed)
}

#[derive(Debug)]
pub(crate) struct ThreadImages {
    thread: ThreadId,
    dir: PathBuf,
}

impl ThreadImages {
    pub(crate) fn of(data: &Path, thread: &ThreadId) -> Self {
        Self {
            thread: thread.clone(),
            dir: thread_folder(data, thread),
        }
    }

    pub(crate) async fn deliver(&self, message: &Message) -> Result<UserInput, ImageCopyError> {
        let copies = if message.stored.is_empty() {
            Vec::new()
        } else {
            let (thread, dir, stored) = (
                self.thread.clone(),
                self.dir.clone(),
                message.stored.clone(),
            );
            tokio::task::spawn_blocking(move || copy_all(&thread, &dir, &stored))
                .await
                .map_err(|source| ImageCopyError::Task {
                    thread: self.thread.clone(),
                    source,
                })??
        };
        Ok(UserInput {
            id: message.id.clone(),
            text: with_image_paths(message.text.clone(), &copies),
            images: message.stored.clone(),
        })
    }
}

fn copy_all(
    thread: &ThreadId,
    dir: &Path,
    stored: &[PathBuf],
) -> Result<Vec<PathBuf>, ImageCopyError> {
    stored
        .iter()
        .map(|from| {
            let name = from
                .file_name()
                .ok_or_else(|| ImageCopyError::Unnamed { path: from.clone() })?;
            let to = dir.join(name);
            std::fs::create_dir_all(dir)
                .and_then(|()| copy_pristine(from, &to))
                .map_err(|source| ImageCopyError::Copy {
                    thread: thread.clone(),
                    from: from.clone(),
                    to: to.clone(),
                    source,
                })?;
            Ok(to)
        })
        .collect()
}

fn copy_pristine(from: &Path, to: &Path) -> io::Result<()> {
    let pristine = std::fs::read(from)?;
    let intact = || std::fs::read(to).is_ok_and(|copy| copy == pristine);
    if intact() {
        return Ok(());
    }
    let mut partial = to.as_os_str().to_os_string();
    partial.push(format!(
        ".{}-{}.partial",
        std::process::id(),
        PARTIAL.fetch_add(1, Ordering::Relaxed)
    ));
    let partial = PathBuf::from(partial);
    match std::fs::write(&partial, &pristine).and_then(|()| std::fs::rename(&partial, to)) {
        Ok(()) => Ok(()),
        Err(error) => {
            if let Err(cleanup) = std::fs::remove_file(&partial) {
                tracing::debug!(partial = %partial.display(), %cleanup, "remove the partial image copy");
            }
            if intact() { Ok(()) } else { Err(error) }
        }
    }
}

fn with_image_paths(text: String, images: &[PathBuf]) -> String {
    let lines: Vec<String> = images
        .iter()
        .enumerate()
        .map(|(index, path)| format!("[Image {} is saved at {}]", index + 1, path.display()))
        .collect();
    match (text.is_empty(), lines.is_empty()) {
        (_, true) => text,
        (true, false) => lines.join("\n"),
        (false, false) => format!("{text}\n\n{}", lines.join("\n")),
    }
}
