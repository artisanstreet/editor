use crate::Store;
use anyhow::{Context, Result};
use artisan_api::{ItemId, ThreadId};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

pub const MAX_RESOURCE_SNAPSHOT_BYTES: usize = 32 * 1024 * 1024;

fn file_safe(id: &str) -> bool {
    (1..=128).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

pub fn snapshot_path(
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
            .join(&hex::encode(hasher.finalize())[..32])
    })
}

pub fn keep(target: &Path, bytes: &[u8]) -> Result<bool> {
    if bytes.len() > MAX_RESOURCE_SNAPSHOT_BYTES {
        return Ok(false);
    }
    let folder = target.parent().context("snapshot folder")?;
    fs::create_dir_all(folder).with_context(|| format!("create {}", folder.display()))?;
    let mut temporary = tempfile::NamedTempFile::new_in(folder)?;
    temporary.write_all(bytes)?;
    match temporary.persist_noclobber(target) {
        Ok(_) => Ok(true),
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => {
            Err(error).with_context(|| format!("write resource snapshot {}", target.display()))
        }
    }
}

pub fn gc(dir: &Path, threads: &HashSet<String>) -> Result<usize> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error).with_context(|| format!("list {}", dir.display())),
    };
    let mut removed = 0;
    for entry in entries {
        let entry = entry?;
        let kept = entry
            .file_name()
            .to_str()
            .is_some_and(|name| threads.contains(name));
        if !kept && entry.file_type()?.is_dir() {
            fs::remove_dir_all(entry.path())
                .with_context(|| format!("remove {}", entry.path().display()))?;
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

    pub async fn gc_resource_snapshots(&self) -> Result<usize> {
        let dir = self.inner.resource_dir.clone();
        let threads = self
            .read(|conn| {
                conn.prepare_cached("SELECT id FROM threads")?
                    .query_map([], |row| row.get::<_, String>(0))?
                    .collect::<rusqlite::Result<HashSet<_>>>()
                    .map_err(Into::into)
            })
            .await?;
        tokio::task::spawn_blocking(move || gc(&dir, &threads))
            .await
            .context("resource snapshot collection task")?
    }
}
