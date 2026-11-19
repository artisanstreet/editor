use crate::Store;
use crate::error::{StorageError, operation_error};
use artisan_api::{ItemId, ThreadId};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

pub const MAX_RESOURCE_SNAPSHOT_BYTES: usize = 32 * 1024 * 1024;

type Result<T, E = ResourceFailure> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum ResourceFailure {
    #[error("{step} {}", path.display())]
    Io {
        step: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Storage(#[from] StorageError),
}

fn io(step: &'static str, path: &Path) -> impl FnOnce(std::io::Error) -> ResourceFailure {
    let path = path.to_path_buf();
    move |source| ResourceFailure::Io { step, path, source }
}

operation_error!(
    CollectSnapshotsError(ResourceFailure),
    "collect resource snapshots"
);

fn file_safe(id: &str) -> bool {
    (1..=128).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn snapshot_path(
    dir: &Path,
    thread: &ThreadId,
    item: &ItemId,
    written: &str,
    from_home: bool,
) -> Option<PathBuf> {
    (file_safe(thread.as_str()) && file_safe(item.as_str())).then(|| {
        let mut hasher = Sha256::new();
        hasher.update([u8::from(from_home)]);
        hasher.update(written.as_bytes());
        dir.join(thread.as_str())
            .join(item.as_str())
            .join(hex::encode(&hasher.finalize()[..16]))
    })
}

pub fn keep(target: &Path, bytes: &[u8]) -> Result<bool> {
    if bytes.len() > MAX_RESOURCE_SNAPSHOT_BYTES {
        return Ok(false);
    }
    let folder = target
        .parent()
        .expect("invariant: a snapshot target sits in its item folder");
    fs::create_dir_all(folder).map_err(io("create", folder))?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(folder).map_err(io("create a snapshot file in", folder))?;
    temporary
        .write_all(bytes)
        .map_err(io("write resource snapshot", target))?;
    match temporary.persist_noclobber(target) {
        Ok(_) => Ok(true),
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(io("write resource snapshot", target)(error.error)),
    }
}

fn gc(dir: &Path, threads: &HashSet<String>) -> Result<usize> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(io("list", dir)(error)),
    };
    let mut removed = 0;
    for entry in entries {
        let entry = entry.map_err(io("list", dir))?;
        let kept = entry
            .file_name()
            .to_str()
            .is_some_and(|name| threads.contains(name));
        let is_dir = entry
            .file_type()
            .map_err(io("read the type of", &entry.path()))?
            .is_dir();
        if !kept && is_dir {
            fs::remove_dir_all(entry.path()).map_err(io("remove", &entry.path()))?;
            removed += 1;
        }
    }
    Ok(removed)
}

impl Store {
    #[must_use]
    pub fn resource_snapshot_target(
        &self,
        thread: &ThreadId,
        item: &ItemId,
        written: &str,
        from_home: bool,
    ) -> Option<PathBuf> {
        snapshot_path(&self.inner.resource_dir, thread, item, written, from_home)
    }

    pub async fn gc_resource_snapshots(&self) -> Result<usize, CollectSnapshotsError> {
        let dir = self.inner.resource_dir.clone();
        let collected = async {
            let threads = self
                .read(|conn| {
                    Ok::<_, StorageError>(
                        conn.prepare_cached(include_str!("sql/state/thread_ids.sql"))?
                            .query_map([], |row| row.get::<_, String>(0))?
                            .collect::<rusqlite::Result<HashSet<_>>>()?,
                    )
                })
                .await?;
            tokio::task::spawn_blocking(move || gc(&dir, &threads))
                .await
                .map_err(StorageError::Task)?
        };
        collected.await.map_err(|cause| CollectSnapshotsError {
            cause: Box::new(cause),
        })
    }
}
