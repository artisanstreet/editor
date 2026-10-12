//! One-time, best-effort import of the old Forge database (`forge.sqlite3`
//! from the SeaORM era) into a [`Store`].
//!
//! Projects, threads, user messages (with images, re-encoded to WebP),
//! assistant text, images the engine viewed, the latest run of each thread
//! (for its engine session id) and a few settings come across with their old
//! ids and their old timestamps. Everything else — tool calls and shell output (only in the raw
//! observation log), reasoning, the patch log, receipts, checkpoints, older
//! runs, drafts — is counted and dropped. Rows that fail to map are skipped,
//! never fatal. Running the import again adds nothing: ids are kept and
//! items, runs and settings already present are left alone.

use crate::writer::Stamp;
use crate::{NewItem, Store};
use anyhow::{Context, Result, anyhow};
use artisan_api::{
    BlobHash, CreateProject, CreateThread, EngineConfig, EngineId, ItemId, ItemKind, Permissions,
    Phase, ProjectId, Run, RunId, RunStatus, Status, ThreadId, Timestamp, UpdateThread, Usage,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Titles the old Editor gave untitled threads; the new one derives a title
/// from the first message instead.
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
    let mut import = Import {
        store,
        engine_images: old_data_dir.join("engine-images"),
        report: ImportReport::default(),
        skipped: BTreeMap::new(),
    };

    let projects = old.projects()?;
    let mut known_projects = HashSet::new();
    for project in projects {
        let id = project.id.clone();
        match import.project(project).await {
            Ok(()) => {
                known_projects.insert(id);
            }
            Err(error) => {
                tracing::warn!(project = %id, error = format!("{error:#}"), "skip legacy project");
                import.skip("projects that failed to import", 1);
            }
        }
    }

    let default_config = old.default_config()?;
    let mut viewed = old.viewed_images()?;
    for thread in old.threads()? {
        if !known_projects.contains(&thread.project_id) {
            import.skip("threads of an unknown project", 1);
            continue;
        }
        let id = thread.id.clone();
        let viewed = viewed.remove(&thread.id).unwrap_or_default();
        // Read before awaiting: a borrowed connection would make the future !Send.
        let loaded = old
            .entries(&thread.id, viewed)
            .and_then(|entries| Ok((entries, old.latest_run(&thread.id)?)));
        let result = match loaded {
            Ok((entries, run)) => {
                import
                    .thread(thread, entries, run, default_config.as_ref())
                    .await
            }
            Err(error) => Err(error),
        };
        if let Err(error) = result {
            tracing::warn!(thread = %id, error = format!("{error:#}"), "skip legacy thread");
            import.skip("threads that failed to import", 1);
        }
    }
    let orphaned: usize = viewed.values().map(Vec::len).sum();
    import.skip("viewed images of unknown threads", orphaned);

    for (key, value) in old.settings()? {
        if let Err(error) = import.setting(key.clone(), value).await {
            tracing::warn!(key, error = format!("{error:#}"), "skip legacy setting");
            import.skip("settings that failed to import", 1);
        }
    }

    let prompt = old.prompt()?;
    let versions = prompt.versions.len();
    if prompt.versions.is_empty() && prompt.current.is_none() {
        // Nothing to take over.
    } else if store.import_prompt(prompt.versions, prompt.current).await? {
        import.report.prompt_versions = versions;
    } else {
        import.skip("already imported", 1);
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
                Some(stamp(project.attached_at, project.attached_at)),
            )
            .await?;
        self.report.projects += 1;
        Ok(())
    }

    async fn thread(
        &mut self,
        thread: OldThread,
        entries: Entries,
        run: Option<OldRun>,
        default_config: Option<&EngineConfig>,
    ) -> Result<()> {
        for (reason, count) in entries.skipped {
            self.skip(reason, count);
        }
        if entries.items.is_empty() {
            self.skip("threads without messages", 1);
            return Ok(());
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
                    },
                    Some(stamp(thread.created_at, thread.created_at)),
                )
                .await?;
            self.report.threads += 1;
        }

        for entry in entries.items {
            if self.store.item(&entry.id).await?.is_some() {
                self.skip("already imported", 1);
                continue;
            }
            let Some(kind) = self.kind(entry.content).await? else {
                continue;
            };
            self.store
                .upsert_item_at(
                    NewItem {
                        id: entry.id,
                        thread_id: thread.id.clone(),
                        run_id: entry.run_id,
                        parent_id: None,
                        native_id: entry.native_id,
                        status: entry.status,
                        kind,
                    },
                    Some(stamp(entry.created_at, entry.updated_at)),
                )
                .await?;
            self.report.items += 1;
        }

        if let Some(run) = run {
            if self.store.run(&run.run.id).await?.is_some() {
                self.skip("already imported", 1);
            } else {
                self.store.upsert_run(run.run).await?;
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
                    Some(stamp(thread.created_at, thread.updated_at)),
                )
                .await?;
        }
        Ok(())
    }

    /// `None` when the entry turned out to have nothing to show.
    async fn kind(&mut self, content: Content) -> Result<Option<ItemKind>> {
        Ok(Some(match content {
            Content::User {
                text,
                images,
                steer,
            } => {
                let mut hashes = Vec::with_capacity(images.len());
                for image in images {
                    if let Some(hash) = self.blob(image.bytes, "message images").await? {
                        hashes.push(hash);
                    }
                }
                if text.trim().is_empty() && hashes.is_empty() {
                    self.skip("empty user messages", 1);
                    return Ok(None);
                }
                ItemKind::UserMessage {
                    text,
                    images: hashes,
                    steer,
                }
            }
            Content::Assistant { text, phase } => ItemKind::AssistantMessage { text, phase },
            Content::Viewed { file_name, caption } => {
                let path = self.engine_images.join(&file_name);
                let Ok(bytes) = std::fs::read(&path) else {
                    self.skip("viewed images missing on disk", 1);
                    return Ok(None);
                };
                let Some(blob) = self.blob(bytes, "viewed images").await? else {
                    return Ok(None);
                };
                ItemKind::Image { blob, caption }
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

struct Old {
    conn: Connection,
    tables: HashSet<String>,
}

/// The old global prompt: `(revision, text, saved at)` per saved version,
/// and the text engines got, if the user had settled one.
#[derive(Default)]
struct OldPrompt {
    versions: Vec<(u64, String, Timestamp)>,
    current: Option<String>,
}

struct OldProject {
    id: ProjectId,
    path: String,
    name: Option<String>,
    attached_at: Timestamp,
}

struct OldThread {
    id: ThreadId,
    project_id: ProjectId,
    title: Option<String>,
    config: Option<EngineConfig>,
    archived: bool,
    read: bool,
    created_at: Timestamp,
    updated_at: Timestamp,
}

struct OldRun {
    run: Run,
    config: Option<EngineConfig>,
}

#[derive(Default)]
struct Entries {
    items: Vec<Entry>,
    skipped: Vec<(&'static str, usize)>,
}

struct Entry {
    id: ItemId,
    run_id: Option<RunId>,
    native_id: Option<String>,
    status: Status,
    created_at: Timestamp,
    updated_at: Timestamp,
    content: Content,
}

enum Content {
    User {
        text: String,
        images: Vec<OldImage>,
        steer: bool,
    },
    Assistant {
        text: String,
        phase: Phase,
    },
    Viewed {
        file_name: String,
        caption: Option<String>,
    },
}

struct OldImage {
    bytes: Vec<u8>,
}

/// An image the engine looked at (Claude's Read on a picture), recorded only
/// in the observation log with a path under `engine-images/`.
struct ViewedImage {
    id: ItemId,
    run_id: Option<RunId>,
    at: Timestamp,
    file_name: String,
    caption: Option<String>,
}

/// One conversation row before it becomes an [`Entry`].
struct Row {
    id: String,
    turn_id: Option<String>,
    item_kind: String,
    phase: Option<String>,
    lifecycle: String,
    body: String,
    run_id: Option<String>,
    native_id: Option<String>,
    source_message_id: Option<String>,
    created_at: Timestamp,
    updated_at: Timestamp,
}

impl Old {
    fn open(path: &Path) -> Result<Self> {
        // Plain read-only: the old Forge may still be writing (WAL), so no
        // `immutable`.
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .with_context(|| format!("open old database {} read-only", path.display()))?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let tables = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")?
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()
            .with_context(|| format!("list tables of {}", path.display()))?;
        Ok(Self { conn, tables })
    }

    fn has_column(&self, table: &str, column: &str) -> Result<bool> {
        Ok(self
            .conn
            .prepare("SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2")?
            .exists([table, column])?)
    }

    /// The global prompt: every saved version, and the text the old Forge
    /// gave the engines once the user had settled it (`established`; old
    /// databases without that column count a non-empty text as settled).
    fn prompt(&self) -> Result<OldPrompt> {
        let mut prompt = OldPrompt::default();
        if self.has("global_prompt_versions") {
            prompt.versions = self
                .conn
                .prepare(
                    "SELECT revision, text, saved_at_ms FROM global_prompt_versions
                     ORDER BY revision",
                )?
                .query_map([], |row| {
                    Ok((
                        u64::try_from(row.get::<_, i64>(0)?).unwrap_or(0),
                        row.get(1)?,
                        row.get(2)?,
                    ))
                })?
                .collect::<rusqlite::Result<_>>()
                .context("read old global prompt versions")?;
        }
        if self.has("global_prompt") {
            let established = if self.has_column("global_prompt", "established")? {
                "established = 1"
            } else {
                "text != ''"
            };
            prompt.current = self
                .conn
                .query_row(
                    &format!(
                        "SELECT text FROM global_prompt WHERE {established}
                         ORDER BY state_id LIMIT 1"
                    ),
                    [],
                    |row| row.get(0),
                )
                .optional()
                .context("read old global prompt")?;
        }
        Ok(prompt)
    }

    fn has(&self, table: &str) -> bool {
        self.tables.contains(table)
    }

    fn projects(&self) -> Result<Vec<OldProject>> {
        if !self.has("attached_projects") {
            return Ok(Vec::new());
        }
        let mut statement = self.conn.prepare(
            "SELECT project_id, root_path, display_name, attached_at_ms FROM attached_projects
             ORDER BY attached_at_ms",
        )?;
        let projects = statement
            .query_map([], |row| {
                Ok(OldProject {
                    id: ProjectId(row.get(0)?),
                    path: row.get(1)?,
                    name: row
                        .get::<_, Option<String>>(2)?
                        .filter(|name| !name.is_empty()),
                    attached_at: row.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()
            .context("read old projects")?;
        Ok(projects)
    }

    /// Oldest activity first, so the new activity order follows the old one.
    fn threads(&self) -> Result<Vec<OldThread>> {
        if !self.has("threads") {
            return Ok(Vec::new());
        }
        let archived = if self.has("thread_lifecycle") {
            "(SELECT archived_at_ms IS NOT NULL FROM thread_lifecycle l
                WHERE l.thread_id = t.thread_id)"
        } else {
            "0"
        };
        let read = if self.has("thread_reads") {
            "(SELECT read_at_ms >= t.updated_at_ms FROM thread_reads r
                WHERE r.thread_id = t.thread_id)"
        } else {
            "0"
        };
        let sql = format!(
            "SELECT thread_id, project_id, title, engine_run_config,
                    coalesce({archived}, 0), coalesce({read}, 0), created_at_ms, updated_at_ms
             FROM threads t ORDER BY updated_at_ms, rowid"
        );
        let mut statement = self.conn.prepare(&sql)?;
        let threads = statement
            .query_map([], |row| {
                let config: Option<Vec<u8>> = row.get(3)?;
                Ok(OldThread {
                    id: ThreadId(row.get(0)?),
                    project_id: ProjectId(row.get(1)?),
                    title: row.get(2)?,
                    config: config.as_deref().and_then(engine_config),
                    archived: row.get(4)?,
                    read: row.get(5)?,
                    created_at: row.get(6)?,
                    updated_at: row.get(7)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()
            .context("read old threads")?;
        Ok(threads)
    }

    /// The thread's conversation in its old order, with viewed images slotted
    /// in by time.
    fn entries(&self, thread_id: &ThreadId, viewed: Vec<ViewedImage>) -> Result<Entries> {
        let mut entries = Entries::default();
        if !self.has("conversation_items") {
            return Ok(entries);
        }
        let rows = self
            .rows(thread_id)
            .with_context(|| format!("read conversation of thread {thread_id}"))?;

        // Old items without a phase: the last assistant text of a turn is
        // the reply, the rest is commentary.
        let mut last_of_turn: HashMap<&str, &str> = HashMap::new();
        for row in &rows {
            if row.item_kind == "assistant_message" {
                last_of_turn.insert(row.turn_id.as_deref().unwrap_or(""), &row.id);
            }
        }

        let mut viewed = viewed.into_iter().peekable();
        let mut other_kinds: BTreeMap<String, usize> = BTreeMap::new();
        let mut empty_assistant = 0;
        for row in &rows {
            while let Some(image) = viewed.next_if(|image| image.at < row.created_at) {
                entries.items.push(viewed_entry(image));
            }
            let content = match row.item_kind.as_str() {
                "user_message" => Content::User {
                    text: row.body.clone(),
                    images: match &row.source_message_id {
                        Some(message_id) => self.message_images(message_id)?,
                        None => Vec::new(),
                    },
                    steer: match &row.source_message_id {
                        Some(message_id) => self.was_steer(message_id)?,
                        None => false,
                    },
                },
                "assistant_message" => {
                    if row.body.trim().is_empty() {
                        empty_assistant += 1;
                        continue;
                    }
                    let phase = match row.phase.as_deref() {
                        Some("final") => Phase::Final,
                        Some("commentary") => Phase::Commentary,
                        _ if last_of_turn.get(row.turn_id.as_deref().unwrap_or(""))
                            == Some(&row.id.as_str()) =>
                        {
                            Phase::Final
                        }
                        _ => Phase::Commentary,
                    };
                    Content::Assistant {
                        text: row.body.clone(),
                        phase,
                    }
                }
                other => {
                    *other_kinds.entry(other.to_owned()).or_default() += 1;
                    continue;
                }
            };
            entries.items.push(Entry {
                id: ItemId(row.id.clone()),
                run_id: row.run_id.clone().map(RunId),
                native_id: row.native_id.clone(),
                status: item_status(&row.lifecycle),
                created_at: row.created_at,
                updated_at: row.updated_at,
                content,
            });
        }
        entries.items.extend(viewed.map(viewed_entry));
        if empty_assistant > 0 {
            entries
                .skipped
                .push(("empty assistant messages", empty_assistant));
        }
        let other: usize = other_kinds.values().sum();
        if other > 0 {
            entries
                .skipped
                .push(("conversation items of other kinds", other));
        }
        Ok(entries)
    }

    fn rows(&self, thread_id: &ThreadId) -> Result<Vec<Row>> {
        let mut statement = self.conn.prepare(
            "SELECT item_id, turn_id, item_kind, phase, lifecycle, coalesce(body, ''), run_id,
                    native_item_key, source_message_id, created_at_ms, updated_at_ms
             FROM conversation_items
             WHERE thread_id = ?1 AND kind = 'item'
             ORDER BY ordinal",
        )?;
        let rows = statement
            .query_map([thread_id.as_str()], |row| {
                Ok(Row {
                    id: row.get(0)?,
                    turn_id: row.get(1)?,
                    item_kind: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    phase: row.get(3)?,
                    lifecycle: row.get(4)?,
                    body: row.get(5)?,
                    run_id: row.get(6)?,
                    native_id: row.get(7)?,
                    source_message_id: row.get(8)?,
                    created_at: row.get(9)?,
                    updated_at: row.get(10)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    fn message_images(&self, message_id: &str) -> Result<Vec<OldImage>> {
        if !self.has("message_image_attachments") {
            return Ok(Vec::new());
        }
        let mut statement = self.conn.prepare_cached(
            "SELECT bytes FROM message_image_attachments WHERE message_id = ?1 ORDER BY position",
        )?;
        let images = statement
            .query_map([message_id], |row| Ok(OldImage { bytes: row.get(0)? }))?
            .collect::<rusqlite::Result<_>>()
            .with_context(|| format!("read images of message {message_id}"))?;
        Ok(images)
    }

    fn was_steer(&self, message_id: &str) -> Result<bool> {
        if !self.has("message_dispatches") {
            return Ok(false);
        }
        let steer = self
            .conn
            .prepare_cached(
                "SELECT steer_run_id IS NOT NULL FROM message_dispatches WHERE message_id = ?1",
            )?
            .query_row([message_id], |row| row.get(0))
            .optional()
            .with_context(|| format!("read dispatch of message {message_id}"))?;
        Ok(steer.unwrap_or(false))
    }

    /// The thread's latest run, carrying the newest engine session id the
    /// thread had (the latest run may have failed before binding one).
    fn latest_run(&self, thread_id: &ThreadId) -> Result<Option<OldRun>> {
        if !self.has("assistant_runs") {
            return Ok(None);
        }
        let row = self
            .conn
            .query_row(
                "SELECT run_id, lifecycle, provider_binding, error_code, error_message,
                        created_at_ms, terminal_at_ms, engine_run_config
                 FROM assistant_runs WHERE thread_id = ?1
                 ORDER BY created_at_ms DESC, rowid DESC LIMIT 1",
                [thread_id.as_str()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<Vec<u8>>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Timestamp>(5)?,
                        row.get::<_, Option<Timestamp>>(6)?,
                        row.get::<_, Option<Vec<u8>>>(7)?,
                    ))
                },
            )
            .optional()
            .with_context(|| format!("read latest run of thread {thread_id}"))?;
        let Some((id, lifecycle, binding, error_code, error_message, started_at, ended_at, config)) =
            row
        else {
            return Ok(None);
        };
        let mut binding = binding.as_deref().and_then(json_object);
        if binding
            .as_ref()
            .and_then(|binding| binding.get("session_id"))
            .is_none()
        {
            let earlier: Option<Vec<u8>> = self
                .conn
                .query_row(
                    "SELECT provider_binding FROM assistant_runs
                     WHERE thread_id = ?1 AND provider_binding IS NOT NULL
                     ORDER BY created_at_ms DESC, rowid DESC LIMIT 1",
                    [thread_id.as_str()],
                    |row| row.get(0),
                )
                .optional()?;
            binding = earlier.as_deref().and_then(json_object).or(binding);
        }
        let session_id = binding
            .as_ref()
            .and_then(|binding| binding.get("session_id"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let config = config.as_deref().and_then(engine_config);
        let (usage, usage_model) = self.usage(&id)?;
        let engine = config
            .as_ref()
            .map(|config| config.engine)
            .or_else(|| {
                binding
                    .as_ref()
                    .and_then(|binding| binding.get("engine"))
                    .and_then(Value::as_str)
                    .and_then(|engine| engine.parse().ok())
            })
            .unwrap_or(EngineId::Claude);
        let run = Run {
            id: RunId(id),
            thread_id: thread_id.clone(),
            engine,
            model: config
                .as_ref()
                .and_then(|config| config.model.clone())
                .or(usage_model),
            status: run_status(&lifecycle),
            usage,
            session_id,
            error: error_message.or(error_code),
            seq: 0,
            started_at,
            ended_at: ended_at.or(Some(started_at)),
        };
        Ok(Some(OldRun { run, config }))
    }

    fn usage(&self, run_id: &str) -> Result<(Usage, Option<String>)> {
        if !self.has("run_usage") {
            return Ok((Usage::default(), None));
        }
        let row = self
            .conn
            .query_row(
                "SELECT input_tokens, cached_input_tokens, output_tokens, context_tokens,
                        context_window_tokens, model_id
                 FROM run_usage WHERE run_id = ?1
                 ORDER BY source_sequence DESC LIMIT 1",
                [run_id],
                |row| {
                    let tokens = |index| -> rusqlite::Result<Option<u64>> {
                        Ok(row
                            .get::<_, Option<i64>>(index)?
                            .and_then(|value| u64::try_from(value).ok()))
                    };
                    Ok((
                        Usage {
                            input: tokens(0)?.unwrap_or(0),
                            cache_read: tokens(1)?.unwrap_or(0),
                            output: tokens(2)?.unwrap_or(0),
                            context_used: tokens(3)?,
                            context_window: tokens(4)?,
                            ..Usage::default()
                        },
                        row.get::<_, Option<String>>(5)?,
                    ))
                },
            )
            .optional()
            .with_context(|| format!("read usage of run {run_id}"))?;
        Ok(row.unwrap_or_default())
    }

    /// Image observations by thread, oldest first.
    fn viewed_images(&self) -> Result<HashMap<ThreadId, Vec<ViewedImage>>> {
        let mut by_thread: HashMap<ThreadId, Vec<ViewedImage>> = HashMap::new();
        if !self.has("observation_ledger") {
            return Ok(by_thread);
        }
        let mut statement = self.conn.prepare(
            "SELECT thread_id, run_id, committed_at_ms, observation_bytes FROM observation_ledger
             WHERE observation_tag = 'image' ORDER BY committed_at_ms, delivery_sequence",
        )?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let thread_id = ThreadId(row.get(0)?);
            let run_id: Option<String> = row.get(1)?;
            let at: Timestamp = row.get(2)?;
            let bytes: Vec<u8> = row.get(3)?;
            let Ok(batch) = serde_json::from_slice::<Value>(&bytes) else {
                continue;
            };
            let observations = batch.get("observations").and_then(Value::as_array);
            for observation in observations.into_iter().flatten() {
                if observation.get("tag").and_then(Value::as_str) != Some("image") {
                    continue;
                }
                let (Some(id), Some(path)) = (
                    observation.get("id").and_then(Value::as_str),
                    observation.get("path").and_then(Value::as_str),
                ) else {
                    continue;
                };
                // By name, so a copied data directory works too.
                let Some(file_name) = Path::new(path).file_name() else {
                    continue;
                };
                by_thread
                    .entry(thread_id.clone())
                    .or_default()
                    .push(ViewedImage {
                        id: ItemId(id.to_owned()),
                        run_id: run_id.clone().map(RunId),
                        at,
                        file_name: file_name.to_string_lossy().into_owned(),
                        caption: observation
                            .get("caption")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                    });
            }
        }
        Ok(by_thread)
    }

    fn default_config(&self) -> Result<Option<EngineConfig>> {
        if !self.has("user_preferences") {
            return Ok(None);
        }
        let blob: Option<Vec<u8>> = self
            .conn
            .query_row(
                "SELECT default_engine_config FROM user_preferences ORDER BY state_id LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .context("read old default engine config")?
            .flatten();
        Ok(blob.as_deref().and_then(engine_config))
    }

    /// Settings as the new store keys them.
    fn settings(&self) -> Result<Vec<(String, Value)>> {
        let mut settings = Vec::new();
        if let Some(config) = self.default_config()? {
            settings.push((
                "default_engine_config".to_owned(),
                serde_json::to_value(config)?,
            ));
        }
        if self.has("model_preferences") {
            let mut statement = self.conn.prepare(
                "SELECT model_id, engine_config FROM model_preferences ORDER BY recency DESC",
            )?;
            let mut configs = Map::new();
            let rows = statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
                .context("read old model preferences")?;
            for (model, blob) in rows {
                if let Some(config) = engine_config(&blob) {
                    configs.insert(model, serde_json::to_value(config)?);
                }
            }
            if !configs.is_empty() {
                settings.push(("model_engine_configs".to_owned(), Value::Object(configs)));
            }
        }
        if self.has("model_favorites") {
            let favourites: Vec<String> = self
                .conn
                .prepare("SELECT model_id FROM model_favorites ORDER BY favorited_at_ms")?
                .query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()
                .context("read old model favourites")?;
            if !favourites.is_empty() {
                settings.push(("favourite_models".to_owned(), json!(favourites)));
            }
        }
        Ok(settings)
    }

    /// Row counts of what the import leaves behind on purpose.
    fn dropped_tables(&self) -> Result<Vec<(&'static str, usize)>> {
        let mut dropped = Vec::new();
        let mut count = |reason: &'static str, table: &str, filter: &str| -> Result<()> {
            if self.has(table) {
                let rows: i64 = self
                    .conn
                    .query_row(
                        &format!("SELECT count(*) FROM {table} {filter}"),
                        [],
                        |row| row.get(0),
                    )
                    .with_context(|| format!("count {table}"))?;
                dropped.push((reason, usize::try_from(rows).unwrap_or(0)));
            }
            Ok(())
        };
        count(
            "observations (tool calls, shell output, reasoning)",
            "observation_ledger",
            "WHERE observation_tag IS NOT 'image'",
        )?;
        count("patch log rows", "conversation_patches", "")?;
        count(
            "older runs",
            "assistant_runs",
            "WHERE rowid NOT IN (SELECT rowid FROM assistant_runs a WHERE created_at_ms =
                (SELECT max(created_at_ms) FROM assistant_runs b WHERE b.thread_id = a.thread_id))",
        )?;
        count(
            "messages never shown (queued or withdrawn)",
            "messages",
            "WHERE message_id NOT IN (SELECT source_message_id FROM conversation_items
                WHERE source_message_id IS NOT NULL)",
        )?;
        count("composer drafts", "composer_drafts", "")?;
        count("composer uploads", "composer_attachments", "")?;
        count("run checkpoints", "run_checkpoints", "")?;
        count("pending interactions", "pending_run_interactions", "")?;
        Ok(dropped)
    }
}

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

fn json_object(bytes: &[u8]) -> Option<Map<String, Value>> {
    match serde_json::from_slice(bytes) {
        Ok(Value::Object(object)) => Some(object),
        _ => None,
    }
}

/// The old versioned engine config (`{engine, permission, details, …}`).
fn engine_config(bytes: &[u8]) -> Option<EngineConfig> {
    let old = json_object(bytes)?;
    let engine: EngineId = old.get("engine")?.as_str()?.parse().ok()?;
    let text = |object: Option<&Value>, key: &str| {
        object
            .and_then(|object| object.get(key))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let permission = old.get("permission");
    let details = old.get("details");
    let mut config = EngineConfig::new(engine);
    config.model = text(details, "model_id");
    config.effort = text(details, "effort").or_else(|| text(details, "reasoning_effort"));
    let never_asks = text(permission, "approval").as_deref() == Some("never")
        || text(details, "permission_mode").as_deref() == Some("bypassPermissions");
    config.permissions = if never_asks {
        Permissions::Full
    } else {
        Permissions::Auto
    };
    if let Some(tier) = text(details, "service_tier") {
        config
            .options
            .insert("service_tier".to_owned(), Value::String(tier));
    }
    Some(config)
}

fn item_status(lifecycle: &str) -> Status {
    match lifecycle {
        // Streaming rows of finished runs never got their final lifecycle;
        // their text is what the user saw.
        "completed" | "streaming" => Status::Done,
        "failed" => Status::Failed,
        _ => Status::Cancelled,
    }
}

fn run_status(lifecycle: &str) -> RunStatus {
    match lifecycle {
        "completed" => RunStatus::Done,
        "failed" => RunStatus::Failed,
        "cancelled" => RunStatus::Cancelled,
        // Anything still live belonged to the old process.
        _ => RunStatus::Interrupted,
    }
}
