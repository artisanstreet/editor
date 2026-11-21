#[cfg(test)]
#[path = "../tests/images/unit.rs"]
mod tests;

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
    #[error(transparent)]
    Folder(#[from] FolderError),
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum RootError {
    #[error("check the image cache {}", path.display())]
    Check {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("the image cache {} is not a plain folder, so Forge leaves it alone", path.display())]
    NotAFolder { path: PathBuf },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum FolderError {
    #[error(transparent)]
    Root(#[from] RootError),
    #[error("create the image folder {} of thread {thread}", path.display())]
    Create {
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
    #[error(transparent)]
    Root(#[from] RootError),
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum RemoveImagesError {
    #[error("remove the image copies at {}", path.display())]
    Remove {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{} is not a plain folder, so Forge leaves the image copies beneath it", path.display())]
    NotAFolder { path: PathBuf },
    #[error(transparent)]
    Root(#[from] RootError),
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

#[derive(Debug)]
struct Root(PathBuf);

impl Root {
    fn existing(data: &Path) -> Result<Option<Self>, RootError> {
        let path = data.join(ROOT);
        match std::fs::symlink_metadata(&path) {
            Ok(entry) if entry.file_type().is_dir() => Ok(Some(Self(path))),
            Ok(_) => Err(RootError::NotAFolder { path }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(RootError::Check { path, source }),
        }
    }

    fn ensure(data: &Path) -> Result<Self, RootError> {
        let path = data.join(ROOT);
        std::fs::create_dir_all(&path).map_err(|source| RootError::Check {
            path: path.clone(),
            source,
        })?;
        Self::existing(data)?.ok_or_else(|| RootError::Check {
            path,
            source: io::ErrorKind::NotFound.into(),
        })
    }

    fn recheck(&self) -> Result<(), RootError> {
        match std::fs::symlink_metadata(&self.0) {
            Ok(entry) if entry.file_type().is_dir() => Ok(()),
            Ok(_) => Err(RootError::NotAFolder {
                path: self.0.clone(),
            }),
            Err(source) => Err(RootError::Check {
                path: self.0.clone(),
                source,
            }),
        }
    }

    fn remove(&self, beneath: &[&std::ffi::OsStr]) -> Result<bool, RemoveImagesError> {
        self.recheck()?;
        let Some((last, parents)) = beneath.split_last() else {
            return Ok(false);
        };
        let mut path = self.0.clone();
        for parent in parents {
            path.push(parent);
            match std::fs::symlink_metadata(&path) {
                Ok(entry) if entry.file_type().is_dir() => {}
                Ok(_) => return Err(RemoveImagesError::NotAFolder { path }),
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
                Err(source) => return Err(RemoveImagesError::Remove { path, source }),
            }
        }
        path.push(last);
        remove_entry(&path)
    }
}

fn plain_folder(
    thread: &ThreadId,
    dir: &Path,
    entry: io::Result<std::fs::Metadata>,
) -> Result<(), FolderError> {
    let entry = entry.map_err(|source| FolderError::Create {
        thread: thread.clone(),
        path: dir.to_path_buf(),
        source,
    })?;
    if entry.file_type().is_dir() {
        Ok(())
    } else {
        Err(FolderError::NotAFolder {
            thread: thread.clone(),
            path: dir.to_path_buf(),
        })
    }
}

fn ensure_folder(data: &Path, thread: &ThreadId) -> Result<PathBuf, FolderError> {
    let root = Root::ensure(data)?;
    let dir = thread_folder(data, thread);
    let entry = match std::fs::create_dir(&dir) {
        Ok(()) => std::fs::symlink_metadata(&dir),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            std::fs::symlink_metadata(&dir)
        }
        Err(error) => Err(error),
    };
    plain_folder(thread, &dir, entry)?;
    root.recheck()?;
    Ok(dir)
}

pub(crate) async fn grantable_folder(
    data: &Path,
    thread: &ThreadId,
) -> Result<Result<PathBuf, FolderError>, tokio::task::JoinError> {
    let (data, thread) = (data.to_path_buf(), thread.clone());
    tokio::task::spawn_blocking(move || ensure_folder(&data, &thread)).await
}

fn remove_entry(path: &Path) -> Result<bool, RemoveImagesError> {
    let removed = match std::fs::symlink_metadata(path) {
        Ok(entry) if entry.file_type().is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(error) => Err(error),
    };
    match removed {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(RemoveImagesError::Remove {
            path: path.to_path_buf(),
            source,
        }),
    }
}

pub(crate) fn remove_thread_folder(
    data: &Path,
    thread: &ThreadId,
) -> Result<(), RemoveImagesError> {
    let Some(root) = Root::existing(data)? else {
        return Ok(());
    };
    let key = crate::scratch::folder_key(thread);
    root.remove(&[key.as_ref()]).map(drop)
}

fn swept(root: &Root, beneath: &[&std::ffi::OsStr]) -> usize {
    match root.remove(beneath) {
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
    match std::fs::symlink_metadata(folder) {
        Ok(entry) if entry.file_type().is_dir() => {}
        Ok(_) => return Vec::new(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => {
            tracing::warn!(folder = %folder.display(), %error, "check a live thread's image folder; its partial copies stay");
            return Vec::new();
        }
    }
    let entries = match std::fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(error) => {
            tracing::warn!(folder = %folder.display(), %error, "list a live thread's image copies");
            return Vec::new();
        }
    };
    entries
        .filter_map(|entry| match entry {
            Ok(entry) => Some(entry),
            Err(error) => {
                tracing::warn!(folder = %folder.display(), %error, "list a live thread's image copies");
                None
            }
        })
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .ends_with(PARTIAL_SUFFIX)
        })
        .map(|entry| entry.path())
        .collect()
}

pub(crate) async fn sweep(
    data: &Path,
    store: &Store,
    _exclusive: &DataLock,
) -> Result<usize, SweepError> {
    let Some(root) = Root::existing(data)? else {
        return Ok(0);
    };
    let listed = std::fs::read_dir(&root.0)
        .and_then(|entries| {
            entries
                .map(|entry| entry.map(|entry| entry.file_name()))
                .collect::<io::Result<Vec<_>>>()
        })
        .map_err(|source| SweepError::List {
            path: root.0.clone(),
            source,
        })?;
    let live: HashSet<String> = store
        .thread_ids()
        .await?
        .iter()
        .map(crate::scratch::folder_key)
        .collect();
    let mut removed = 0;
    for name in listed {
        root.recheck()?;
        removed += if live.contains(name.to_string_lossy().as_ref()) {
            partials(&root.0.join(&name))
                .iter()
                .filter_map(|partial| partial.file_name())
                .map(|partial| swept(&root, &[name.as_os_str(), partial]))
                .sum()
        } else {
            swept(&root, &[name.as_os_str()])
        };
    }
    Ok(removed)
}

#[derive(Debug)]
pub(crate) struct ThreadImages {
    thread: ThreadId,
    data: PathBuf,
}

impl ThreadImages {
    pub(crate) fn of(data: &Path, thread: &ThreadId) -> Self {
        Self {
            thread: thread.clone(),
            data: data.to_path_buf(),
        }
    }

    pub(crate) async fn deliver(&self, message: &Message) -> UserInput {
        let copies = if message.stored.is_empty() {
            Vec::new()
        } else {
            let (thread, data, stored) = (
                self.thread.clone(),
                self.data.clone(),
                message.stored.clone(),
            );
            match tokio::task::spawn_blocking(move || copy_all(&thread, &data, &stored)).await {
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

fn copy_all(thread: &ThreadId, data: &Path, stored: &[PathBuf]) -> Vec<Option<PathBuf>> {
    stored
        .iter()
        .map(|from| {
            copy_one(thread, data, from)
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

fn copy_one(thread: &ThreadId, data: &Path, from: &Path) -> Result<PathBuf, ImageCopyError> {
    let name = from.file_name().ok_or_else(|| ImageCopyError::Unnamed {
        path: from.to_path_buf(),
    })?;
    let dir = ensure_folder(data, thread)?;
    let to = dir.join(name);
    copy_pristine(from, &dir, &to).map_err(|source| ImageCopyError::Copy {
        thread: thread.clone(),
        from: from.to_path_buf(),
        to: to.clone(),
        source,
    })?;
    Ok(to)
}

fn copy_pristine(from: &Path, dir: &Path, to: &Path) -> io::Result<()> {
    let pristine = std::fs::read(from)?;
    if intact(to, &pristine) {
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
            if intact(to, &pristine) {
                Ok(())
            } else {
                Err(error.error)
            }
        }
    }
}

fn intact(copy: &Path, pristine: &[u8]) -> bool {
    match std::fs::symlink_metadata(copy) {
        Ok(entry) if entry.file_type().is_file() => {}
        Ok(_) => return false,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return false,
        Err(error) => {
            tracing::warn!(copy = %copy.display(), %error, "check an image copy; copying it again");
            return false;
        }
    }
    match std::fs::read(copy) {
        Ok(bytes) => bytes == pristine,
        Err(error) => {
            tracing::warn!(copy = %copy.display(), %error, "read an image copy; copying it again");
            false
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
