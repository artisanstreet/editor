//! Read-only, bounded access to the retired SeaORM database.
use super::config::{engine_config, item_status, json_object, run_status};
use super::{IMPORT_BYTES, IMPORT_ROWS, viewed_entry};
use anyhow::{Context, Result};
use artisan_api::{
    EngineConfig, EngineId, ItemId, Phase, ProjectId, Run, RunId, Status, ThreadId, Timestamp,
    Usage,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::path::Path;

pub(super) struct Old {
    pub(super) conn: Connection,
    pub(super) tables: HashSet<String>,
}

pub(super) struct OldProject {
    pub(super) rowid: i64,
    pub(super) id: ProjectId,
    pub(super) path: String,
    pub(super) name: Option<String>,
    pub(super) attached_at: Timestamp,
}

pub(super) struct OldThread {
    pub(super) rowid: i64,
    pub(super) id: ThreadId,
    pub(super) project_id: ProjectId,
    pub(super) title: Option<String>,
    pub(super) config: Option<EngineConfig>,
    pub(super) archived: bool,
    pub(super) read: bool,
    pub(super) created_at: Timestamp,
    pub(super) updated_at: Timestamp,
}

pub(super) struct OldRun {
    pub(super) run: Run,
    pub(super) config: Option<EngineConfig>,
}

#[derive(Default)]
pub(super) struct Entries {
    pub(super) items: Vec<Entry>,
    pub(super) skipped: Vec<(&'static str, usize)>,
}

pub(super) struct EntryPage {
    pub(super) entries: Entries,
    pub(super) next: Option<(i64, i64)>,
}

pub(super) struct Entry {
    pub(super) id: ItemId,
    pub(super) run_id: Option<RunId>,
    pub(super) native_id: Option<String>,
    pub(super) status: Status,
    pub(super) created_at: Timestamp,
    pub(super) updated_at: Timestamp,
    pub(super) content: Content,
}

pub(super) enum Content {
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

pub(super) struct OldImage {
    pub(super) rowid: i64,
}

/// An image the engine looked at (Claude's Read on a picture), recorded only
/// in the observation log with a path under `engine-images/`.
pub(super) struct ViewedImage {
    pub(super) id: ItemId,
    pub(super) run_id: Option<RunId>,
    pub(super) at: Timestamp,
    pub(super) file_name: String,
    pub(super) caption: Option<String>,
}

/// One conversation row before it becomes an [`Entry`].
pub(super) struct ViewedImages<'a> {
    pub(super) old: &'a Old,
    pub(super) thread_id: ThreadId,
    pub(super) cursor: Option<(i64, i64, i64, i64)>,
    pub(super) buffer: VecDeque<ViewedImage>,
    pub(super) exhausted: bool,
}

impl<'a> ViewedImages<'a> {
    pub(super) fn new(old: &'a Old, thread_id: ThreadId) -> Self {
        Self {
            old,
            thread_id,
            cursor: None,
            buffer: VecDeque::new(),
            exhausted: !old.has("observation_ledger"),
        }
    }

    pub(super) fn peek(&mut self) -> Result<Option<&ViewedImage>> {
        if self.buffer.is_empty() && !self.exhausted {
            let mut query = self.old.conn.prepare_cached("SELECT o.committed_at_ms, o.delivery_sequence, o.rowid, CAST(j.key AS INTEGER),
                   json_extract(j.value,'$.id'), o.run_id, json_extract(j.value,'$.path'), json_extract(j.value,'$.caption')
                 FROM observation_ledger o, json_each(
                   CASE WHEN length(o.observation_bytes) <= ?6 AND json_valid(CAST(o.observation_bytes AS TEXT))
                     THEN CAST(o.observation_bytes AS TEXT) ELSE '{\"observations\":[]}' END, '$.observations') j
                 WHERE o.thread_id = ?1 AND o.observation_tag = 'image' AND json_extract(j.value,'$.tag') = 'image'
                   AND json_type(j.value,'$.id') = 'text' AND json_type(j.value,'$.path') = 'text'
                   AND (?2 IS NULL OR (o.committed_at_ms,o.delivery_sequence,o.rowid,CAST(j.key AS INTEGER)) > (?2,?3,?4,?5))
                 ORDER BY o.committed_at_ms,o.delivery_sequence,o.rowid,CAST(j.key AS INTEGER) LIMIT 256")?;
            let mut rows = query.query_map(
                rusqlite::params![
                    self.thread_id.as_str(),
                    self.cursor.map(|v| v.0),
                    self.cursor.map(|v| v.1),
                    self.cursor.map(|v| v.2),
                    self.cursor.map(|v| v.3),
                    IMPORT_BYTES
                ],
                |row| {
                    Ok((
                        (row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?),
                        ViewedImage {
                            id: ItemId(row.get(4)?),
                            run_id: row.get::<_, Option<String>>(5)?.map(RunId),
                            at: row.get(0)?,
                            file_name: Path::new(&row.get::<_, String>(6)?)
                                .file_name()
                                .map(|name| name.to_string_lossy().into_owned())
                                .unwrap_or_default(),
                            caption: row.get(7)?,
                        },
                    ))
                },
            )?;
            let mut bytes = 0;
            while let Some(row) = rows.next() {
                let (cursor, image) = row?;
                bytes += image.file_name.len() + image.caption.as_ref().map_or(0, String::len);
                self.cursor = Some(cursor);
                self.buffer.push_back(image);
                if bytes >= IMPORT_BYTES {
                    break;
                }
            }
            if self.buffer.is_empty() {
                self.exhausted = true;
            }
        }
        Ok(self.buffer.front())
    }

    pub(super) fn next(&mut self) -> Result<Option<ViewedImage>> {
        self.peek()?;
        Ok(self.buffer.pop_front())
    }
}

pub(super) struct Row {
    pub(super) ordinal: i64,
    pub(super) rowid: i64,
    pub(super) id: String,
    pub(super) turn_id: Option<String>,
    pub(super) item_kind: String,
    pub(super) phase: Option<String>,
    pub(super) lifecycle: String,
    pub(super) body: String,
    pub(super) run_id: Option<String>,
    pub(super) native_id: Option<String>,
    pub(super) source_message_id: Option<String>,
    pub(super) created_at: Timestamp,
    pub(super) updated_at: Timestamp,
}

impl Old {
    pub(super) fn open(path: &Path) -> Result<Self> {
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

    pub(super) fn has_column(&self, table: &str, column: &str) -> Result<bool> {
        Ok(self
            .conn
            .prepare("SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2")?
            .exists([table, column])?)
    }

    /// Stream saved native revisions in source order, retaining one finite row.
    pub(super) fn for_each_prompt(
        &self,
        mut put: impl FnMut(u64, String, Timestamp) -> Result<()>,
    ) -> Result<usize> {
        if !self.has("global_prompt_versions") {
            return Ok(0);
        }
        let mut query=self.conn.prepare_cached("SELECT revision,length(CAST(text AS BLOB)),text,saved_at_ms FROM global_prompt_versions ORDER BY revision")?;
        let mut rows = query.query([])?;
        let mut count = 0;
        while let Some(row) = rows.next()? {
            let bytes: usize = row.get(1)?;
            anyhow::ensure!(
                bytes <= 32 * 1024 * 1024,
                "old prompt version exceeds 32 MiB"
            );
            put(
                u64::try_from(row.get::<_, i64>(0)?).unwrap_or(0),
                row.get(2)?,
                row.get(3)?,
            )?;
            count += 1;
        }
        Ok(count)
    }

    pub(super) fn prompt_current(&self) -> Result<Option<String>> {
        if !self.has("global_prompt") {
            return Ok(None);
        }
        let established = if self.has_column("global_prompt", "established")? {
            "established=1"
        } else {
            "text != ''"
        };
        let mut query=self.conn.prepare_cached(&format!("SELECT length(CAST(text AS BLOB)),text FROM global_prompt WHERE {established} ORDER BY state_id LIMIT 1"))?;
        let mut rows = query.query([])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        anyhow::ensure!(
            row.get::<_, usize>(0)? <= 32 * 1024 * 1024,
            "old current prompt exceeds 32 MiB"
        );
        Ok(Some(row.get(1)?))
    }

    pub(super) fn has(&self, table: &str) -> bool {
        self.tables.contains(table)
    }

    pub(super) fn projects(&self, cursor: Option<(Timestamp, i64)>) -> Result<Vec<OldProject>> {
        if !self.has("attached_projects") {
            return Ok(Vec::new());
        }
        let mut statement = self.conn.prepare(
            "SELECT project_id, root_path, display_name, attached_at_ms, rowid FROM attached_projects
             WHERE length(CAST(root_path AS BLOB)) + length(CAST(coalesce(display_name,'') AS BLOB)) <= 4194304 AND (?1 IS NULL OR (attached_at_ms, rowid) > (?1, ?2)) ORDER BY attached_at_ms, rowid LIMIT 256",
        )?;
        let source = statement.query_map(
            rusqlite::params![cursor.map(|value| value.0), cursor.map(|value| value.1)],
            |row| {
                Ok(OldProject {
                    rowid: row.get(4)?,
                    id: ProjectId(row.get(0)?),
                    path: row.get(1)?,
                    name: row
                        .get::<_, Option<String>>(2)?
                        .filter(|name| !name.is_empty()),
                    attached_at: row.get(3)?,
                })
            },
        )?;
        let mut rows = Vec::new();
        let mut bytes = 0;
        for row in source {
            let row = row.context("read old projects")?;
            bytes +=
                row.id.as_str().len() + row.path.len() + row.name.as_ref().map_or(0, String::len);
            rows.push(row);
            if bytes >= IMPORT_BYTES {
                break;
            }
        }
        Ok(rows)
    }

    /// Oldest activity first, so the new activity order follows the old one.
    pub(super) fn threads(&self, cursor: Option<(Timestamp, i64)>) -> Result<Vec<OldThread>> {
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
                    coalesce({archived}, 0), coalesce({read}, 0), created_at_ms, updated_at_ms, t.rowid
             FROM threads t WHERE length(CAST(coalesce(title,'') AS BLOB)) + length(CAST(coalesce(engine_run_config,'') AS BLOB)) <= 4194304 AND (?1 IS NULL OR (updated_at_ms, t.rowid) > (?1, ?2)) ORDER BY updated_at_ms, t.rowid LIMIT 256"
        );
        let mut statement = self.conn.prepare(&sql)?;
        let source = statement.query_map(
            rusqlite::params![cursor.map(|value| value.0), cursor.map(|value| value.1)],
            |row| {
                let config: Option<Vec<u8>> = row.get(3)?;
                Ok(OldThread {
                    rowid: row.get(8)?,
                    id: ThreadId(row.get(0)?),
                    project_id: ProjectId(row.get(1)?),
                    title: row.get(2)?,
                    config: config.as_deref().and_then(engine_config),
                    archived: row.get(4)?,
                    read: row.get(5)?,
                    created_at: row.get(6)?,
                    updated_at: row.get(7)?,
                })
            },
        )?;
        let mut rows = Vec::new();
        let mut bytes = 0;
        for row in source {
            let row = row.context("read old threads")?;
            bytes += row.id.as_str().len()
                + row.project_id.as_str().len()
                + row.title.as_ref().map_or(0, String::len)
                + row
                    .config
                    .as_ref()
                    .map(|value| serde_json::to_vec(value).map(|bytes| bytes.len()))
                    .transpose()?
                    .unwrap_or(0);
            rows.push(row);
            if bytes >= IMPORT_BYTES {
                break;
            }
        }
        Ok(rows)
    }

    /// The thread's conversation in its old order, with viewed images slotted
    /// in by time.
    pub(super) fn entries(
        &self,
        thread_id: &ThreadId,
        viewed: &mut ViewedImages<'_>,
        cursor: Option<(i64, i64)>,
    ) -> Result<EntryPage> {
        let mut entries = Entries::default();
        if !self.has("conversation_items") {
            for _ in 0..IMPORT_ROWS {
                let Some(image) = viewed.next()? else {
                    break;
                };
                entries.items.push(viewed_entry(image));
            }
            let next = viewed
                .peek()?
                .is_some()
                .then_some(cursor.unwrap_or((i64::MIN, i64::MIN)));
            return Ok(EntryPage { entries, next });
        }
        let rows = self
            .rows(thread_id, cursor)
            .with_context(|| format!("read conversation of thread {thread_id}"))?;

        let mut next = cursor;
        let mut image_bytes = 0;
        let mut other_kinds: BTreeMap<String, usize> = BTreeMap::new();
        let mut empty_assistant = 0;
        for row in &rows {
            while viewed
                .peek()?
                .is_some_and(|image| image.at < row.created_at)
            {
                let image = viewed.next()?.expect("peeked image");
                image_bytes +=
                    image.file_name.len() + image.caption.as_ref().map_or(0, String::len);
                entries.items.push(viewed_entry(image));
                if entries.items.len() >= IMPORT_ROWS || image_bytes >= IMPORT_BYTES {
                    return Ok(EntryPage {
                        entries,
                        next: Some(next.unwrap_or((i64::MIN, i64::MIN))),
                    });
                }
            }
            next = Some((row.ordinal, row.rowid));
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
                        _ if self.is_last_assistant(thread_id, row)? => Phase::Final,
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
        if rows.is_empty() {
            for _ in 0..IMPORT_ROWS {
                let Some(image) = viewed.next()? else {
                    break;
                };
                image_bytes +=
                    image.file_name.len() + image.caption.as_ref().map_or(0, String::len);
                entries.items.push(viewed_entry(image));
                if image_bytes >= IMPORT_BYTES {
                    break;
                }
            }
            next = viewed
                .peek()?
                .is_some()
                .then_some(cursor.unwrap_or((i64::MIN, i64::MIN)));
        }
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
        Ok(EntryPage { entries, next })
    }

    pub(super) fn is_last_assistant(&self, thread_id: &ThreadId, row: &Row) -> Result<bool> {
        let last: String = self.conn.prepare_cached("SELECT item_id FROM conversation_items WHERE thread_id = ?1 AND coalesce(turn_id, '') = ?2 AND item_kind = 'assistant_message' AND kind = 'item' ORDER BY ordinal DESC, rowid DESC LIMIT 1")?.query_row(rusqlite::params![thread_id.as_str(), row.turn_id.as_deref().unwrap_or("")], |row| row.get(0))?;
        Ok(last == row.id)
    }

    pub(super) fn rows(
        &self,
        thread_id: &ThreadId,
        cursor: Option<(i64, i64)>,
    ) -> Result<Vec<Row>> {
        let mut statement = self.conn.prepare(
            "SELECT item_id, turn_id, item_kind, phase, lifecycle, coalesce(body, ''), run_id,
                    native_item_key, source_message_id, created_at_ms, updated_at_ms, ordinal, rowid
             FROM conversation_items
             WHERE thread_id = ?1 AND kind = 'item'
               AND (?2 IS NULL OR (ordinal, rowid) > (?2, ?3))
             ORDER BY ordinal, rowid LIMIT 256",
        )?;
        let mut source = statement.query(rusqlite::params![
            thread_id.as_str(),
            cursor.map(|cursor| cursor.0),
            cursor.map(|cursor| cursor.1)
        ])?;
        let mut rows = Vec::new();
        let mut bytes = 0;
        while let Some(row) = source.next()? {
            let body: String = row.get(5)?;
            bytes += body.len();
            rows.push(Row {
                id: row.get(0)?,
                turn_id: row.get(1)?,
                item_kind: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                phase: row.get(3)?,
                lifecycle: row.get(4)?,
                body,
                run_id: row.get(6)?,
                native_id: row.get(7)?,
                source_message_id: row.get(8)?,
                created_at: row.get(9)?,
                updated_at: row.get(10)?,
                ordinal: row.get(11)?,
                rowid: row.get(12)?,
            });
            if bytes >= IMPORT_BYTES {
                break;
            }
        }
        Ok(rows)
    }

    pub(super) fn message_images(&self, message_id: &str) -> Result<Vec<OldImage>> {
        if !self.has("message_image_attachments") {
            return Ok(Vec::new());
        }
        let mut statement = self.conn.prepare_cached(
            "SELECT rowid FROM message_image_attachments WHERE message_id = ?1 ORDER BY position, rowid LIMIT 129",
        )?;
        let images: Vec<OldImage> = statement
            .query_map([message_id], |row| Ok(OldImage { rowid: row.get(0)? }))?
            .collect::<rusqlite::Result<_>>()
            .with_context(|| format!("read images of message {message_id}"))?;
        anyhow::ensure!(
            images.len() <= 128,
            "old message {message_id} exceeds 128 image references; none of its attachments were silently dropped"
        );
        Ok(images)
    }

    pub(super) fn image_bytes(&self, rowid: i64) -> Result<Option<Vec<u8>>> {
        Ok(self.conn.prepare_cached("SELECT CASE WHEN length(bytes) <= ?2 THEN bytes ELSE NULL END FROM message_image_attachments WHERE rowid = ?1")?.query_row(rusqlite::params![rowid, crate::blobs::MAX_BLOB_BYTES], |row| row.get(0))?)
    }

    pub(super) fn was_steer(&self, message_id: &str) -> Result<bool> {
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
    pub(super) fn latest_run(&self, thread_id: &ThreadId) -> Result<Option<OldRun>> {
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
            input_cutoff: 0, // Imported displayed user messages are terminal; none are pending.
            started_at,
            ended_at: ended_at.or(Some(started_at)),
        };
        Ok(Some(OldRun { run, config }))
    }

    pub(super) fn usage(&self, run_id: &str) -> Result<(Usage, Option<String>)> {
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
    pub(super) fn default_config(&self) -> Result<Option<EngineConfig>> {
        if !self.has("user_preferences") {
            return Ok(None);
        }
        let mut query=self.conn.prepare_cached("SELECT length(CAST(default_engine_config AS BLOB)),default_engine_config FROM user_preferences ORDER BY state_id LIMIT 1")?;
        let mut rows = query.query([])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        anyhow::ensure!(
            row.get::<_, Option<usize>>(0)?
                .is_none_or(|size| size <= 32 * 1024 * 1024),
            "old default engine config exceeds 32 MiB"
        );
        let blob: Option<Vec<u8>> = row.get(1)?;
        Ok(blob.as_deref().and_then(engine_config))
    }

    /// Settings as the new store keys them.
    pub(super) fn settings(&self) -> Result<Vec<(String, Value)>> {
        let mut settings = Vec::new();
        if let Some(config) = self.default_config()? {
            settings.push((
                "default_engine_config".to_owned(),
                serde_json::to_value(config)?,
            ));
        }
        if self.has("model_preferences") {
            let mut statement = self.conn.prepare(
                "SELECT length(CAST(model_id AS BLOB)),length(CAST(engine_config AS BLOB)),model_id,engine_config FROM model_preferences ORDER BY recency DESC",
            )?;
            let mut configs = Map::new();
            let mut rows = statement.query([])?;
            let mut bytes = 2usize;
            while let Some(row) = rows.next()? {
                let native_bytes = row
                    .get::<_, usize>(0)?
                    .saturating_add(row.get::<_, usize>(1)?);
                anyhow::ensure!(
                    native_bytes <= 32 * 1024 * 1024,
                    "old model preference row exceeds 32 MiB"
                );
                let model: String = row.get(2)?;
                let blob: Vec<u8> = row.get(3)?;
                if let Some(config) = engine_config(&blob) {
                    let value = serde_json::to_value(config)?;
                    bytes = bytes
                        .saturating_add(serde_json::to_vec(&model)?.len())
                        .saturating_add(serde_json::to_vec(&value)?.len())
                        .saturating_add(2);
                    anyhow::ensure!(
                        bytes <= 32 * 1024 * 1024,
                        "old model_engine_configs map exceeds 32 MiB"
                    );
                    configs.insert(model, value);
                }
            }
            if !configs.is_empty() {
                settings.push(("model_engine_configs".to_owned(), Value::Object(configs)));
            }
        }
        if self.has("model_favorites") {
            let mut query=self.conn.prepare_cached("SELECT length(CAST(model_id AS BLOB)),model_id FROM model_favorites ORDER BY favorited_at_ms")?;
            let mut rows = query.query([])?;
            let mut favourites = Vec::new();
            let mut bytes = 2usize;
            while let Some(row) = rows.next()? {
                anyhow::ensure!(
                    row.get::<_, usize>(0)? <= 32 * 1024 * 1024,
                    "old favourite model ID exceeds 32 MiB"
                );
                let model: String = row.get(1)?;
                bytes = bytes
                    .saturating_add(serde_json::to_vec(&model)?.len())
                    .saturating_add(1);
                anyhow::ensure!(
                    bytes <= 32 * 1024 * 1024,
                    "old favourite_models list exceeds 32 MiB"
                );
                favourites.push(model);
            }
            if !favourites.is_empty() {
                settings.push(("favourite_models".to_owned(), json!(favourites)));
            }
        }
        Ok(settings)
    }

    /// Row counts of what the import leaves behind on purpose.
    pub(super) fn dropped_tables(&self) -> Result<Vec<(&'static str, usize)>> {
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
        count(
            "oversized viewed-image observation batches",
            "observation_ledger",
            "WHERE observation_tag = 'image' AND length(observation_bytes) > 4194304",
        )?;
        count(
            "viewed-image batches of unknown threads",
            "observation_ledger",
            "WHERE observation_tag = 'image' AND thread_id NOT IN (SELECT thread_id FROM threads)",
        )?;
        count(
            "oversized project metadata rows",
            "attached_projects",
            "WHERE length(CAST(root_path AS BLOB)) + length(CAST(coalesce(display_name,'') AS BLOB)) > 4194304",
        )?;
        count(
            "oversized thread metadata rows",
            "threads",
            "WHERE length(CAST(coalesce(title,'') AS BLOB)) + length(CAST(coalesce(engine_run_config,'') AS BLOB)) > 4194304",
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

#[cfg(test)]
mod bounded_metadata_tests {
    use super::*;
    #[test]
    fn metadata_pages_keep_all_rows_across_equal_activity_cursors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.sqlite3");
        let mut conn = Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE attached_projects(project_id TEXT,root_path TEXT,display_name TEXT,attached_at_ms INTEGER); CREATE TABLE threads(thread_id TEXT,project_id TEXT,title TEXT,engine_run_config BLOB,created_at_ms INTEGER,updated_at_ms INTEGER);").unwrap();
        let tx = conn.transaction().unwrap();
        let name = "x".repeat(20 * 1024);
        for n in 0..603 {
            tx.execute(
                "INSERT INTO attached_projects VALUES(?1,'/work',?2,1)",
                rusqlite::params![format!("p-{n}"), name],
            )
            .unwrap();
            tx.execute(
                "INSERT INTO threads VALUES(?1,?2,?3,NULL,1,1)",
                rusqlite::params![format!("t-{n}"), format!("p-{n}"), name],
            )
            .unwrap();
        }
        tx.commit().unwrap();
        drop(conn);
        let old = Old::open(&path).unwrap();
        let mut cursor = None;
        let mut projects = 0;
        loop {
            let page = old.projects(cursor).unwrap();
            if page.is_empty() {
                break;
            }
            assert!(page.len() <= IMPORT_ROWS);
            projects += page.len();
            let last = page.last().unwrap();
            cursor = Some((last.attached_at, last.rowid));
        }
        let mut cursor = None;
        let mut threads = 0;
        loop {
            let page = old.threads(cursor).unwrap();
            if page.is_empty() {
                break;
            }
            assert!(page.len() <= IMPORT_ROWS);
            threads += page.len();
            let last = page.last().unwrap();
            cursor = Some((last.updated_at, last.rowid));
        }
        assert_eq!((projects, threads), (603, 603));
    }
    #[test]
    fn viewed_image_batches_remain_bounded_even_without_conversation_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.sqlite3");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE observation_ledger(thread_id TEXT,run_id TEXT,committed_at_ms INTEGER,delivery_sequence INTEGER,observation_tag TEXT,observation_bytes BLOB);").unwrap();
        let observations=(0..603).map(|n|json!({"tag":"image","id":format!("image-{n}"),"path":format!("/old/image-{n}.png")})).collect::<Vec<_>>();
        conn.execute(
            "INSERT INTO observation_ledger VALUES('thread',NULL,1,1,'image',?1)",
            [serde_json::to_vec(&json!({"observations":observations})).unwrap()],
        )
        .unwrap();
        drop(conn);
        let old = Old::open(&path).unwrap();
        let thread = ThreadId::from("thread");
        let mut images = ViewedImages::new(&old, thread.clone());
        let mut cursor = None;
        let mut ids = Vec::new();
        loop {
            let page = old.entries(&thread, &mut images, cursor).unwrap();
            assert!(page.entries.items.len() <= IMPORT_ROWS);
            ids.extend(page.entries.items.into_iter().map(|item| item.id));
            let Some(next) = page.next else {
                break;
            };
            cursor = Some(next);
        }
        assert_eq!(
            ids,
            (0..603)
                .map(|n| ItemId(format!("image-{n}")))
                .collect::<Vec<_>>()
        );
    }
}

#[cfg(test)]
mod old_attachment_limit_tests {
    use super::*;
    #[test]
    fn excess_native_attachment_metadata_is_rejected_without_reading_blob_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.sqlite3");
        let mut conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE message_image_attachments(message_id TEXT,position INTEGER,bytes BLOB);",
        )
        .unwrap();
        let tx = conn.transaction().unwrap();
        for position in 0..128 {
            tx.execute(
                "INSERT INTO message_image_attachments VALUES('message',?1,NULL)",
                [position],
            )
            .unwrap();
        }
        tx.commit().unwrap();
        let old = Old::open(&path).unwrap();
        assert_eq!(old.message_images("message").unwrap().len(), 128);
        conn.execute(
            "INSERT INTO message_image_attachments VALUES('message',128,NULL)",
            [],
        )
        .unwrap();
        let error = old.message_images("message").err().unwrap();
        assert!(error.to_string().contains("exceeds 128 image references"));
    }
}

#[cfg(test)]
mod utf8_metadata_budget_tests {
    use super::*;
    #[test]
    fn unicode_metadata_limits_count_encoded_bytes_and_report_rejected_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.sqlite3");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE attached_projects(project_id TEXT,root_path TEXT,display_name TEXT,attached_at_ms INTEGER);CREATE TABLE threads(thread_id TEXT,project_id TEXT,title TEXT,engine_run_config BLOB,created_at_ms INTEGER,updated_at_ms INTEGER);").unwrap();
        let text = "€".repeat(IMPORT_BYTES / 3 + 1);
        assert!(text.len() > IMPORT_BYTES);
        assert!(text.chars().count() < IMPORT_BYTES);
        conn.execute(
            "INSERT INTO attached_projects VALUES('project','/work',?1,1)",
            [&text],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO threads VALUES('thread','project',?1,NULL,1,1)",
            [&text],
        )
        .unwrap();
        drop(conn);
        let old = Old::open(&path).unwrap();
        assert!(old.projects(None).unwrap().is_empty());
        assert!(old.threads(None).unwrap().is_empty());
        let skipped = old.dropped_tables().unwrap();
        assert!(skipped.contains(&("oversized project metadata rows", 1)));
        assert!(skipped.contains(&("oversized thread metadata rows", 1)));
    }
}
