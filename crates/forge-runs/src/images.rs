use artisan_api::{ItemId, RunId, ThreadId};
use forge_engines::UserInput;
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

const ROOT: &str = "run-images";

#[derive(Debug, thiserror::Error)]
pub(crate) enum RunImagesError {
    #[error("create the image folder {} of run {run}", path.display())]
    Create {
        run: RunId,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("copy the image {} to {} for run {run}", from.display(), to.display())]
    Copy {
        run: RunId,
        from: PathBuf,
        to: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("the stored image {} has no file name", path.display())]
    Unnamed { path: PathBuf },
    #[error("run the image copy task of run {run}")]
    Task {
        run: RunId,
        #[source]
        source: tokio::task::JoinError,
    },
}

#[derive(Debug, thiserror::Error)]
#[error("clear the run image folders in {}", path.display())]
pub(crate) struct SweepError {
    path: PathBuf,
    #[source]
    source: io::Error,
}

#[derive(Debug, thiserror::Error)]
#[error("create the image folder {} of thread {thread}", path.display())]
pub struct ImageFolderError {
    thread: ThreadId,
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

fn thread_folder(data: &Path, thread: &ThreadId) -> PathBuf {
    data.join(ROOT).join(thread.as_str())
}

pub(crate) async fn readable_folder(
    data: &Path,
    thread: &ThreadId,
) -> Result<PathBuf, ImageFolderError> {
    let path = thread_folder(data, thread);
    tokio::fs::create_dir_all(&path)
        .await
        .map_err(|source| ImageFolderError {
            thread: thread.clone(),
            path: path.clone(),
            source,
        })?;
    Ok(path)
}

pub(crate) fn sweep(data: &Path) -> Result<(), SweepError> {
    let path = data.join(ROOT);
    match std::fs::remove_dir_all(&path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(SweepError {
            path,
            source: error,
        }),
        Ok(()) | Err(_) => Ok(()),
    }
}

#[derive(Debug)]
pub(crate) struct RunImages {
    run: RunId,
    dir: PathBuf,
    copies: HashMap<PathBuf, PathBuf>,
}

impl RunImages {
    pub(crate) async fn open(
        data: &Path,
        thread: &ThreadId,
        run: &RunId,
    ) -> Result<Self, RunImagesError> {
        let dir = thread_folder(data, thread).join(run.as_str());
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|source| RunImagesError::Create {
                run: run.clone(),
                path: dir.clone(),
                source,
            })?;
        Ok(Self {
            run: run.clone(),
            dir,
            copies: HashMap::new(),
        })
    }

    pub(crate) async fn deliver(&mut self, message: Message) -> Result<UserInput, RunImagesError> {
        let Message { id, text, stored } = message;
        let missing: Vec<(PathBuf, PathBuf)> = stored
            .iter()
            .filter(|from| !self.copies.contains_key(*from))
            .map(|from| {
                let name = from
                    .file_name()
                    .ok_or_else(|| RunImagesError::Unnamed { path: from.clone() })?;
                Ok((from.clone(), self.dir.join(name)))
            })
            .collect::<Result<_, RunImagesError>>()?;
        if !missing.is_empty() {
            let run = self.run.clone();
            let copied = tokio::task::spawn_blocking(move || copy_all(&run, missing))
                .await
                .map_err(|source| RunImagesError::Task {
                    run: self.run.clone(),
                    source,
                })??;
            self.copies.extend(copied);
        }
        let images: Vec<PathBuf> = stored
            .iter()
            .map(|from| {
                self.copies
                    .get(from)
                    .cloned()
                    .unwrap_or_else(|| unreachable!("invariant: every stored image was copied"))
            })
            .collect();
        Ok(UserInput {
            id,
            text: with_image_paths(text, &images),
            images,
        })
    }
}

impl Drop for RunImages {
    fn drop(&mut self) {
        match std::fs::remove_dir_all(&self.dir) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => {
                tracing::warn!(run = %self.run, folder = %self.dir.display(), %error, "remove the run's image copies");
            }
            Ok(()) | Err(_) => {}
        }
    }
}

fn copy_all(
    run: &RunId,
    missing: Vec<(PathBuf, PathBuf)>,
) -> Result<Vec<(PathBuf, PathBuf)>, RunImagesError> {
    for (from, to) in &missing {
        std::fs::copy(from, to).map_err(|source| RunImagesError::Copy {
            run: run.clone(),
            from: from.clone(),
            to: to.clone(),
            source,
        })?;
    }
    Ok(missing)
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
