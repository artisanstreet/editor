use artisan_api::{ItemId, ThreadId};
use forge_engines::UserInput;
use forge_store::{DataLock, Store, ThreadIdsError};
use std::collections::HashSet;
use std::io;
use std::io::Write as _;
use std::path::{Path, PathBuf};

const ROOT: &str = "run-images";
const PARTIAL_SUFFIX: &str = ".partial";

#[derive(Debug, thiserror::Error)]
enum ImageCopyError {
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
    #[error("create the image folder {} of thread {thread}", path.display())]
    Folder {
        thread: ThreadId,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("the image folder {} of thread {thread} is not a plain folder", path.display())]
    NotAFolder { thread: ThreadId, path: PathBuf },
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
}

#[derive(Debug, thiserror::Error)]
#[error("remove the image copies at {}", path.display())]
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
}

pub(crate) fn thread_folder(data: &Path, thread: &ThreadId) -> PathBuf {
    data.join(ROOT).join(crate::scratch::folder_key(thread))
}

fn remove_entry(path: &Path) -> Result<bool, RemoveImagesError> {
    let failed = |source| RemoveImagesError {
        path: path.to_path_buf(),
        source,
    };
    let removed = match std::fs::symlink_metadata(path) {
        Ok(entry) if entry.file_type().is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(error) => Err(error),
    };
    match removed {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(failed(error)),
    }
}

pub(crate) fn remove_thread_folder(
    data: &Path,
    thread: &ThreadId,
) -> Result<(), RemoveImagesError> {
    remove_entry(&thread_folder(data, thread)).map(drop)
}

fn swept(path: &Path) -> usize {
    match remove_entry(path) {
        Ok(removed) => usize::from(removed),
        Err(error) => {
            tracing::warn!(
                error = diagnostics::chain(&error),
                "the sweep left an image copy"
            );
            0
        }
    }
}

fn partials(folder: &Path) -> Vec<PathBuf> {
    let is_folder =
        std::fs::symlink_metadata(folder).is_ok_and(|folder| folder.file_type().is_dir());
    if !is_folder {
        return Vec::new();
    }
    match std::fs::read_dir(folder) {
        Ok(entries) => entries
            .filter_map(|entry| {
                entry
                    .inspect_err(|error| {
                        tracing::warn!(folder = %folder.display(), %error, "list a live thread's image copies");
                    })
                    .ok()
            })
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .ends_with(PARTIAL_SUFFIX)
            })
            .map(|entry| entry.path())
            .collect(),
        Err(error) => {
            tracing::warn!(folder = %folder.display(), %error, "list a live thread's image copies");
            Vec::new()
        }
    }
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
    let removed = listed
        .into_iter()
        .map(|name| {
            let path = root.join(&name);
            if live.contains(name.to_string_lossy().as_ref()) {
                partials(&path).iter().map(|partial| swept(partial)).sum()
            } else {
                swept(&path)
            }
        })
        .sum();
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

    pub(crate) async fn deliver(&self, message: &Message) -> UserInput {
        let copies = if message.stored.is_empty() {
            Vec::new()
        } else {
            let (thread, dir, stored) = (
                self.thread.clone(),
                self.dir.clone(),
                message.stored.clone(),
            );
            match tokio::task::spawn_blocking(move || copy_all(&thread, &dir, &stored)).await {
                Ok(copies) => copies,
                Err(error) => {
                    tracing::warn!(thread = %self.thread, item = %message.id, %error, "the image copy task stopped");
                    vec![None; message.stored.len()]
                }
            }
        };
        UserInput {
            id: message.id.clone(),
            text: with_image_paths(message.text.clone(), &copies),
            images: message.stored.clone(),
        }
    }
}

fn copy_all(thread: &ThreadId, dir: &Path, stored: &[PathBuf]) -> Vec<Option<PathBuf>> {
    stored
        .iter()
        .map(|from| {
            copy_one(thread, dir, from)
                .inspect_err(|error| {
                    tracing::warn!(
                        %thread,
                        error = diagnostics::chain(error),
                        "the image goes without its file path"
                    );
                })
                .ok()
        })
        .collect()
}

fn copy_one(thread: &ThreadId, dir: &Path, from: &Path) -> Result<PathBuf, ImageCopyError> {
    let name = from.file_name().ok_or_else(|| ImageCopyError::Unnamed {
        path: from.to_path_buf(),
    })?;
    let folder_error = |source| ImageCopyError::Folder {
        thread: thread.clone(),
        path: dir.to_path_buf(),
        source,
    };
    std::fs::create_dir_all(dir).map_err(folder_error)?;
    if !std::fs::symlink_metadata(dir)
        .map_err(folder_error)?
        .file_type()
        .is_dir()
    {
        return Err(ImageCopyError::NotAFolder {
            thread: thread.clone(),
            path: dir.to_path_buf(),
        });
    }
    let to = dir.join(name);
    copy_pristine(from, dir, &to).map_err(|source| ImageCopyError::Copy {
        thread: thread.clone(),
        from: from.to_path_buf(),
        to: to.clone(),
        source,
    })?;
    Ok(to)
}

fn copy_pristine(from: &Path, dir: &Path, to: &Path) -> io::Result<()> {
    let pristine = std::fs::read(from)?;
    let intact = || {
        std::fs::symlink_metadata(to).is_ok_and(|copy| copy.file_type().is_file())
            && std::fs::read(to).is_ok_and(|copy| copy == pristine)
    };
    if intact() {
        return Ok(());
    }
    let mut partial = tempfile::Builder::new()
        .prefix(".")
        .suffix(PARTIAL_SUFFIX)
        .tempfile_in(dir)?;
    partial.write_all(&pristine)?;
    match partial.persist(to) {
        Ok(_) => Ok(()),
        Err(error) => {
            drop(error.file);
            if intact() { Ok(()) } else { Err(error.error) }
        }
    }
}

fn with_image_paths(text: String, images: &[Option<PathBuf>]) -> String {
    let lines: Vec<String> = images
        .iter()
        .enumerate()
        .map(|(index, path)| match path {
            Some(path) => format!("[Image {} is saved at {}]", index + 1, path.display()),
            None => format!("[The file path of image {} isn't available.]", index + 1),
        })
        .collect();
    match (text.is_empty(), lines.is_empty()) {
        (_, true) => text,
        (true, false) => lines.join("\n"),
        (false, false) => format!("{text}\n\n{}", lines.join("\n")),
    }
}
