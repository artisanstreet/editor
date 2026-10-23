//! One-time, best-effort import of the old Forge database (`forge.sqlite3`
//! from the SeaORM era) into a [`Store`].
//!
//! Projects, threads, user messages (with images, re-encoded to WebP),
//! assistant text, images the engine viewed, the latest run of each thread
//! (for its engine session id) and a few settings come across with their old
//! ids and their old timestamps. Everything else — tool calls and shell output (only in the raw
//! observation log), reasoning, the patch log, receipts, checkpoints, older
//! runs, drafts — is counted and dropped. Rows that fail to map are skipped,
//! never fatal. Oversized prompt/settings metadata is rejected with a clear limit error. Running the import again adds nothing: ids are kept and
//! items, runs and settings already present are left alone.

mod config;
mod repair;
mod source;
pub use repair::{inspect_agent_messages, normalize_agent_messages};
use source::*;

use crate::writer::Stamp;
use crate::{NewItem, Store};
use anyhow::{Context, Result, anyhow};
use artisan_api::{
    BlobHash, CreateProject, CreateThread, EngineConfig, EngineId, ItemId, ItemKind, Status,
    ThreadId, Timestamp, UpdateThread,
};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

/// Titles the old Editor gave untitled threads; the new one derives a title
/// from the first message instead.
const IMPORT_ROWS: usize = 256;
const IMPORT_BYTES: usize = 4 * 1024 * 1024;

const PLACEHOLDER_TITLES: [&str; 2] = ["New task", "New thread"];

/// What an import wrote. A second import of the same database reports zeros
/// and lists the existing rows under "already imported".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportReport {
    pub projects: usize,
    pub threads: usize,
    pub items: usize,
    pub runs: usize,
    /// Images stored as blobs (message attachments and viewed images).
    pub images: usize,
    /// Saved versions of the global prompt.
    pub prompt_versions: usize,
    /// Old rows left behind, by reason.
    pub skipped: Vec<(String, usize)>,
}

/// Imports `old_db` (opened read-only, safe while the old Forge runs) into
/// `store`. `old_data_dir` is the old Forge's data directory, which holds
/// `engine-images/`.
pub async fn import(old_db: &Path, old_data_dir: &Path, store: &Store) -> Result<ImportReport> {
    let old = Old::open(old_db)?;
    normalize_agent_messages(store).await?;
    let mut import = Import {
        store,
        engine_images: old_data_dir.join("engine-images"),
        report: ImportReport::default(),
        skipped: BTreeMap::new(),
    };

    let mut project_cursor = None;
    loop {
        let projects = old.projects(project_cursor)?;
        if projects.is_empty() {
            break;
        }
        for project in projects {
            project_cursor = Some((project.attached_at, project.rowid));
            let id = project.id.clone();
            if let Err(error) = import.project(project).await {
                tracing::warn!(project = %id, error = format!("{error:#}"), "skip legacy project");
                import.skip("projects that failed to import", 1);
            }
        }
    }
    let default_config = old.default_config()?;
    let mut thread_cursor = None;
    loop {
        let threads = old.threads(thread_cursor)?;
        if threads.is_empty() {
            break;
        }
        for thread in threads {
            thread_cursor = Some((thread.updated_at, thread.rowid));
            if store.project(&thread.project_id).await?.is_none() {
                import.skip("threads of an unknown project", 1);
                continue;
            }
            let id = thread.id.clone();
            let result = match old.latest_run(&thread.id) {
                Ok(run) => {
                    import
                        .thread(thread, &old, run, default_config.as_ref())
                        .await
                }
                Err(error) => Err(error),
            };
            if let Err(error) = result {
                tracing::warn!(thread = %id, error = format!("{error:#}"), "skip legacy thread");
                import.skip("threads that failed to import", 1);
            }
        }
    }

    for (key, value) in old.settings()? {
        if let Err(error) = import.setting(key.clone(), value).await {
            tracing::warn!(key, error = format!("{error:#}"), "skip legacy setting");
            import.skip("settings that failed to import", 1);
        }
    }

    match store.import_old_prompt(old_db.to_owned()).await? {
        Some(versions) => import.report.prompt_versions = versions,
        None => import.skip("already imported", 1),
    }

    for (reason, count) in old.dropped_tables()? {
        import.skip(reason, count);
    }
    let mut report = import.report;
    report.skipped = import
        .skipped
        .into_iter()
        .filter(|(_, count)| *count > 0)
        .collect();
    Ok(report)
}

impl Store {
    async fn import_old_prompt(&self, old_db: PathBuf) -> Result<Option<usize>> {
        self.write(move |tx| {
            if crate::read::prompt_version(tx)? > 0
                || crate::read::setting(tx, "global_prompt")?.is_some()
            {
                return Ok(None);
            }
            let old = Old::open(&old_db)?;
            let _snapshot = old.conn.unchecked_transaction()?;
            let current = old.prompt_current()?;
            let versions = old.for_each_prompt(|version, text, saved_at| {
                tx.execute(
                    "INSERT INTO prompt_versions(version,text,saved_at) VALUES(?1,?2,?3)",
                    rusqlite::params![i64::try_from(version).unwrap_or(i64::MAX), text, saved_at],
                )?;
                Ok(())
            })?;
            if let Some(text) = current {
                tx.save_setting("global_prompt".into(), serde_json::Value::String(text))?;
            }
            Ok(Some(versions))
        })
        .await
        .context("import old global prompt")
    }

    async fn imported_ids(&self, ids: Vec<ItemId>) -> Result<HashSet<ItemId>> {
        self.read(move |conn| {
            let mut present = HashSet::new();
            let mut query =
                conn.prepare_cached("SELECT EXISTS(SELECT 1 FROM items WHERE id = ?1)")?;
            for id in ids {
                if query.query_row([id.as_str()], |row| row.get::<_, bool>(0))? {
                    present.insert(id);
                }
            }
            Ok(present)
        })
        .await
    }

    async fn import_items(&self, batch: Vec<(NewItem, Stamp)>) -> Result<(usize, usize)> {
        self.write(move |tx| {
            let (mut inserted, mut present) = (0, 0);
            for (new, stamp) in batch {
                if crate::read::item(tx, &new.id)?.is_some() {
                    present += 1;
                    continue;
                }
                tx.with_stamp(stamp, |tx| {
                    let input = matches!(&new.kind, ItemKind::UserMessage { .. });
                    let native_id = new.native_id.clone();
                    let item = tx.save_item(crate::fresh_item(new, stamp.created_at), None)?;
                    tx.set_native_id(&item.id, native_id.as_deref())?;
                    if input {
                        tx.mark_activity(&item.thread_id, item.seq)?;
                    }
                    Ok(item)
                })?;
                inserted += 1;
            }
            Ok((inserted, present))
        })
        .await
    }

    async fn mark_unread_at(&self, id: &ThreadId, stamp: Stamp) -> Result<()> {
        let id = id.clone();
        self.write(move |tx| {
            tx.with_stamp(stamp, |tx| {
                let mut thread =
                    crate::read::thread(tx, &id)?.context("imported thread disappeared")?;
                thread.marked_unread = true;
                tx.save_thread(thread)?;
                Ok(())
            })
        })
        .await
    }
}

struct Import<'a> {
    store: &'a Store,
    engine_images: PathBuf,
    report: ImportReport,
    skipped: BTreeMap<String, usize>,
}

impl Import<'_> {
    fn skip(&mut self, reason: &str, count: usize) {
        *self.skipped.entry(reason.to_owned()).or_default() += count;
    }

    async fn project(&mut self, project: OldProject) -> Result<()> {
        if self.store.project(&project.id).await?.is_some() {
            self.skip("already imported", 1);
            return Ok(());
        }
        self.store
            .create_project_at(
                CreateProject {
                    id: project.id,
                    path: project.path,
                    name: project.name,
                },
                stamp(project.attached_at, project.attached_at),
            )
            .await?;
        self.report.projects += 1;
        Ok(())
    }

    async fn thread(
        &mut self,
        thread: OldThread,
        old: &Old,
        run: Option<OldRun>,
        default_config: Option<&EngineConfig>,
    ) -> Result<()> {
        let mut viewed = ViewedImages::new(old, thread.id.clone());
        let mut cursor = None;
        let mut page = old.entries(&thread.id, &mut viewed, cursor)?;
        while page.entries.items.is_empty() {
            for (reason, count) in page.entries.skipped {
                self.skip(reason, count);
            }
            let Some(next) = page.next else {
                self.skip("threads without messages", 1);
                return Ok(());
            };
            cursor = Some(next);
            page = old.entries(&thread.id, &mut viewed, cursor)?;
        }
        let existing = self.store.thread(&thread.id).await?;
        if existing.is_some() {
            self.skip("already imported", 1);
        } else {
            let config = thread
                .config
                .or_else(|| run.as_ref().and_then(|run| run.config.clone()))
                .or_else(|| default_config.cloned())
                .unwrap_or_else(|| EngineConfig::new(EngineId::Claude));
            let title = thread.title.filter(|title| {
                !title.trim().is_empty() && !PLACEHOLDER_TITLES.contains(&title.as_str())
            });
            self.store
                .create_thread_at(
                    CreateThread {
                        id: thread.id.clone(),
                        project_id: thread.project_id,
                        config,
                        title,
                        parent_thread_id: None,
                        spawned_by: None,
                    },
                    stamp(thread.created_at, thread.created_at),
                )
                .await?;
            self.report.threads += 1;
        }

        loop {
            for (reason, count) in page.entries.skipped {
                self.skip(reason, count);
            }
            let ids = page
                .entries
                .items
                .iter()
                .map(|entry| entry.id.clone())
                .collect();
            let existing_ids = self.store.imported_ids(ids).await?;
            let mut batch = Vec::new();
            let mut bytes = 0;
            for entry in page.entries.items {
                if existing_ids.contains(&entry.id) {
                    self.skip("already imported", 1);
                    continue;
                }
                let Some(kind) = self.kind(entry.content, old).await? else {
                    continue;
                };
                let item_bytes = serde_json::to_vec(&kind)?.len();
                if !batch.is_empty()
                    && (batch.len() == IMPORT_ROWS || bytes + item_bytes > IMPORT_BYTES)
                {
                    let (written, present) =
                        self.store.import_items(std::mem::take(&mut batch)).await?;
                    self.report.items += written;
                    self.skip("already imported", present);
                    bytes = 0;
                }
                bytes += item_bytes;
                batch.push((
                    NewItem {
                        id: entry.id,
                        thread_id: thread.id.clone(),
                        run_id: entry.run_id,
                        parent_id: None,
                        native_id: entry.native_id,
                        status: entry.status,
                        kind,
                    },
                    stamp(entry.created_at, entry.updated_at),
                ));
            }
            if !batch.is_empty() {
                let (written, present) = self.store.import_items(batch).await?;
                self.report.items += written;
                self.skip("already imported", present);
            }
            let Some(next) = page.next else {
                break;
            };
            page = old.entries(&thread.id, &mut viewed, Some(next))?;
        }

        if let Some(run) = run {
            if self.store.run(&run.run.id).await?.is_some() {
                self.skip("already imported", 1);
            } else {
                let activity = stamp(
                    run.run.started_at,
                    run.run.ended_at.unwrap_or(run.run.started_at),
                );
                self.store.upsert_run_at(run.run, activity).await?;
                self.report.runs += 1;
            }
        }

        // Only on first import: later the user's own read/archive state wins.
        // Always saved then, so the thread ends on its old `updated_at`.
        if existing.is_none() {
            let stored = self
                .store
                .thread(&thread.id)
                .await?
                .ok_or_else(|| anyhow!("thread vanished after import"))?;
            let update = UpdateThread {
                archived: thread.archived.then_some(true),
                read_seq: thread.read.then_some(stored.activity_seq),
                ..UpdateThread::default()
            };
            self.store
                .update_thread_at(
                    &thread.id,
                    update,
                    stamp(thread.created_at, thread.updated_at),
                )
                .await?;
            if !thread.read {
                self.store
                    .mark_unread_at(&thread.id, stamp(thread.created_at, thread.updated_at))
                    .await?;
            }
        }
        Ok(())
    }

    /// `None` when the entry turned out to have nothing to show.
    async fn kind(&mut self, content: Content, old: &Old) -> Result<Option<ItemKind>> {
        Ok(Some(match content {
            Content::User {
                text,
                images,
                steer,
            } => {
                let mut hashes = Vec::with_capacity(images.len());
                for image in images {
                    let Some(bytes) = old.image_bytes(image.rowid)? else {
                        self.skip("message images too large", 1);
                        continue;
                    };
                    if let Some(hash) = self.blob(bytes, "message images").await? {
                        hashes.push(hash);
                    }
                }
                if text.trim().is_empty() && hashes.is_empty() {
                    self.skip("empty user messages", 1);
                    return Ok(None);
                }
                ItemKind::UserMessage {
                    text: text.into(),
                    images: hashes,
                    steer,
                    from_agent: None,
                    sender: None,
                }
            }
            Content::Assistant { text, phase } => ItemKind::AssistantMessage {
                text: text.into(),
                phase,
            },
            Content::Viewed { file_name, caption } => {
                let path = self.engine_images.join(&file_name);
                let loaded = tokio::task::spawn_blocking(move || {
                    use std::io::Read as _;
                    let mut bytes = Vec::new();
                    std::fs::File::open(path)?
                        .take((crate::blobs::MAX_BLOB_BYTES + 1) as u64)
                        .read_to_end(&mut bytes)?;
                    Ok::<_, std::io::Error>(bytes)
                })
                .await
                .context("read legacy image task")?;
                let Ok(bytes) = loaded else {
                    self.skip("viewed images missing on disk", 1);
                    return Ok(None);
                };
                let Some(blob) = self.blob(bytes, "viewed images").await? else {
                    return Ok(None);
                };
                ItemKind::Image {
                    blob,
                    caption: caption.map(Into::into),
                }
            }
        }))
    }

    /// Stores the image as WebP; `None` (counted) if it does not decode.
    async fn blob(&mut self, bytes: Vec<u8>, what: &str) -> Result<Option<BlobHash>> {
        let webp = tokio::task::spawn_blocking(move || crate::blobs::to_webp(bytes))
            .await
            .context("image conversion task")?;
        let webp = match webp {
            Ok(webp) => webp,
            Err(error) => {
                tracing::warn!(error = format!("{error:#}"), "skip legacy image");
                self.skip(&format!("{what} that failed to convert"), 1);
                return Ok(None);
            }
        };
        match self.store.put_blob(webp).await {
            Ok(blob) => {
                self.report.images += 1;
                Ok(Some(blob.hash))
            }
            Err(error) if error.downcast_ref::<crate::BlobRejected>().is_some() => {
                tracing::warn!(error = format!("{error:#}"), "skip legacy image");
                self.skip(&format!("{what} too large"), 1);
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    async fn setting(&mut self, key: String, value: Value) -> Result<()> {
        if self.store.setting(&key).await?.is_some() {
            self.skip("already imported", 1);
            return Ok(());
        }
        self.store.set_setting(key, value).await
    }
}

// The old database.

fn viewed_entry(image: ViewedImage) -> Entry {
    Entry {
        id: image.id,
        run_id: image.run_id,
        native_id: None,
        status: Status::Done,
        created_at: image.at,
        updated_at: image.at,
        content: Content::Viewed {
            file_name: image.file_name,
            caption: image.caption,
        },
    }
}

fn stamp(created_at: Timestamp, updated_at: Timestamp) -> Stamp {
    Stamp {
        created_at,
        updated_at,
    }
}
