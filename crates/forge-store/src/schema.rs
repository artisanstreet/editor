use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension};

/// One step of a migration: SQL, or a data translation SQL cannot express.
enum Step {
    Sql(&'static str),
    Rust(fn(&Connection) -> Result<()>),
}

/// Applied in order; `PRAGMA user_version` counts how many have run.
/// Never edit a released entry, append a new one.
const MIGRATIONS: &[&[Step]] = &[
    &[Step::Sql(
        r"
CREATE TABLE projects (
    id   TEXT PRIMARY KEY,
    seq  INTEGER NOT NULL,
    body TEXT NOT NULL
);
CREATE INDEX projects_seq ON projects (seq);

CREATE TABLE threads (
    id   TEXT PRIMARY KEY,
    seq  INTEGER NOT NULL,
    body TEXT NOT NULL
);
CREATE INDEX threads_seq ON threads (seq);

CREATE TABLE items (
    id        TEXT PRIMARY KEY,
    thread_id TEXT NOT NULL,
    position  INTEGER NOT NULL,
    seq       INTEGER NOT NULL,
    run_id    TEXT,
    native_id TEXT,
    status    TEXT NOT NULL,
    body      TEXT NOT NULL
);
CREATE INDEX items_thread_position ON items (thread_id, position);
CREATE INDEX items_seq ON items (seq);
CREATE INDEX items_native ON items (thread_id, native_id) WHERE native_id IS NOT NULL;
CREATE INDEX items_live ON items (status) WHERE status IN ('running', 'awaiting_user');

CREATE TABLE runs (
    id        TEXT PRIMARY KEY,
    thread_id TEXT NOT NULL,
    seq       INTEGER NOT NULL,
    status    TEXT NOT NULL,
    body      TEXT NOT NULL
);
CREATE INDEX runs_thread ON runs (thread_id);
CREATE INDEX runs_seq ON runs (seq);
CREATE INDEX runs_status ON runs (status);

CREATE TABLE kv (
    key   TEXT PRIMARY KEY,
    seq   INTEGER NOT NULL,
    value TEXT NOT NULL
);
CREATE INDEX kv_seq ON kv (seq);

CREATE TABLE drafts (
    scope TEXT PRIMARY KEY,
    body  TEXT NOT NULL
);

CREATE TABLE authorized_keys (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL,
    public_key TEXT NOT NULL UNIQUE,
    created_at INTEGER NOT NULL,
    last_seen  INTEGER
);

CREATE TABLE pair_codes (
    code_sha256 TEXT PRIMARY KEY,
    expires_at  INTEGER NOT NULL
);
",
    )],
    &[Step::Sql(
        r"
-- Values only Forge reads: never in /state or the event stream.
CREATE TABLE secrets (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
INSERT INTO secrets (key, value) SELECT key, value FROM kv WHERE key = 'forge_identity';
DELETE FROM kv WHERE key = 'forge_identity';
",
    )],
    &[Step::Sql(
        r"
-- Every saved version of the global prompt; the latest is also the
-- `global_prompt` setting.
CREATE TABLE prompt_versions (
    version  INTEGER PRIMARY KEY,
    text     TEXT NOT NULL,
    saved_at INTEGER NOT NULL
);
",
    )],
    &[Step::Sql(
        r"
-- Access and approvals became one choice: Auto or Full. What never asked
-- (or bypassed Claude's permissions) stays Full; everything else is Auto.
UPDATE threads SET body = json_remove(
    json_set(body, '$.config.permissions',
        CASE WHEN json_extract(body, '$.config.approvals') = 'never'
               OR json_extract(body, '$.config.options.permission_mode') = 'bypassPermissions'
             THEN 'full' ELSE 'auto' END),
    '$.config.access', '$.config.approvals',
    '$.config.options.permission_mode', '$.config.options.mode');
UPDATE kv SET value = json_remove(
    json_set(value, '$.permissions',
        CASE WHEN json_extract(value, '$.approvals') = 'never'
               OR json_extract(value, '$.options.permission_mode') = 'bypassPermissions'
             THEN 'full' ELSE 'auto' END),
    '$.access', '$.approvals', '$.options.permission_mode', '$.options.mode')
WHERE key = 'default_engine_config' AND json_valid(value);
",
    )],
    &[Step::Sql(
        r"
CREATE INDEX items_awaiting_position ON items(position) WHERE status = 'awaiting_user';
CREATE INDEX items_pending_input ON items(thread_id, position)
    WHERE status = 'running' AND json_extract(body, '$.kind.type') = 'user_message';
",
    )],
    &[Step::Sql(
        r"
ALTER TABLE drafts ADD COLUMN revision INTEGER NOT NULL DEFAULT 1;
CREATE INDEX threads_activity ON threads(COALESCE(json_extract(body,'$.archived'),0),json_extract(body,'$.activity_seq') DESC,id DESC);
CREATE INDEX items_thread_seq ON items(thread_id,seq);
CREATE INDEX items_thread_kind_position ON items(thread_id,json_extract(body,'$.kind.type'),position);
",
    )],
    &[Step::Sql(
        r"
CREATE VIEW thread_search_content AS
 SELECT t.rowid AS rowid, COALESCE(json_extract(t.body,'$.title'),'') || ' ' || COALESCE(json_extract(p.body,'$.name'),'') AS content
 FROM threads t LEFT JOIN projects p ON p.id=json_extract(t.body,'$.project_id');
CREATE VIRTUAL TABLE threads_search USING fts5(content,content='thread_search_content',content_rowid='rowid',tokenize='trigram',columnsize=0);
INSERT INTO threads_search(threads_search) VALUES('rebuild');
CREATE TRIGGER thread_search_insert AFTER INSERT ON threads BEGIN
 INSERT INTO threads_search(rowid,content) SELECT rowid,content FROM thread_search_content WHERE rowid=NEW.rowid;
END;
CREATE TRIGGER thread_search_delete BEFORE DELETE ON threads BEGIN
 INSERT INTO threads_search(threads_search,rowid,content) SELECT 'delete',rowid,content FROM thread_search_content WHERE rowid=OLD.rowid;
END;
CREATE TRIGGER thread_search_before_update BEFORE UPDATE ON threads
 WHEN json_extract(OLD.body,'$.title') IS NOT json_extract(NEW.body,'$.title') OR json_extract(OLD.body,'$.project_id') IS NOT json_extract(NEW.body,'$.project_id') BEGIN
 INSERT INTO threads_search(threads_search,rowid,content) SELECT 'delete',rowid,content FROM thread_search_content WHERE rowid=OLD.rowid;
END;
CREATE TRIGGER thread_search_after_update AFTER UPDATE ON threads
 WHEN json_extract(OLD.body,'$.title') IS NOT json_extract(NEW.body,'$.title') OR json_extract(OLD.body,'$.project_id') IS NOT json_extract(NEW.body,'$.project_id') BEGIN
 INSERT INTO threads_search(rowid,content) SELECT rowid,content FROM thread_search_content WHERE rowid=NEW.rowid;
END;
CREATE TRIGGER project_search_before_update BEFORE UPDATE ON projects
 WHEN json_extract(OLD.body,'$.name') IS NOT json_extract(NEW.body,'$.name') BEGIN
 INSERT INTO threads_search(threads_search,rowid,content) SELECT 'delete',rowid,content FROM thread_search_content
 WHERE rowid IN (SELECT rowid FROM threads WHERE json_extract(body,'$.project_id')=OLD.id);
END;
CREATE TRIGGER project_search_after_update AFTER UPDATE ON projects
 WHEN json_extract(OLD.body,'$.name') IS NOT json_extract(NEW.body,'$.name') BEGIN
 INSERT INTO threads_search(rowid,content) SELECT rowid,content FROM thread_search_content
 WHERE rowid IN (SELECT rowid FROM threads WHERE json_extract(body,'$.project_id')=NEW.id);
END;
",
    )],
    &[Step::Sql(
        r"CREATE INDEX items_thread_parent_position ON items(thread_id,json_extract(body,'$.parent_id'),position);
CREATE INDEX items_agent_recipient_position ON items(thread_id,json_extract(body,'$.kind.tool.recipient'),position)
 WHERE json_extract(body,'$.parent_id') IS NULL AND json_extract(body,'$.kind.tool.kind')='agent_message';",
    )],
    &[
        Step::Sql(
            r"ALTER TABLE items ADD COLUMN edit_added INTEGER;
       ALTER TABLE items ADD COLUMN edit_removed INTEGER;
       CREATE TABLE thread_edit_totals(thread_id TEXT PRIMARY KEY, added INTEGER NOT NULL, removed INTEGER NOT NULL, operations INTEGER NOT NULL);
       CREATE TRIGGER item_edit_insert AFTER INSERT ON items WHEN NEW.edit_added IS NOT NULL BEGIN
         INSERT INTO thread_edit_totals VALUES(NEW.thread_id,NEW.edit_added,NEW.edit_removed,1)
         ON CONFLICT(thread_id) DO UPDATE SET added=added+excluded.added,removed=removed+excluded.removed,operations=operations+1;
       END;
       CREATE TRIGGER item_edit_update AFTER UPDATE OF edit_added,edit_removed ON items
        WHEN OLD.edit_added IS NOT NEW.edit_added OR OLD.edit_removed IS NOT NEW.edit_removed BEGIN
         INSERT INTO thread_edit_totals VALUES(NEW.thread_id,COALESCE(NEW.edit_added,0)-COALESCE(OLD.edit_added,0),
            COALESCE(NEW.edit_removed,0)-COALESCE(OLD.edit_removed,0),(NEW.edit_added IS NOT NULL)-(OLD.edit_added IS NOT NULL))
         ON CONFLICT(thread_id) DO UPDATE SET added=added+excluded.added,removed=removed+excluded.removed,operations=operations+excluded.operations;
       END;
       CREATE TRIGGER item_edit_delete AFTER DELETE ON items WHEN OLD.edit_added IS NOT NULL BEGIN
         UPDATE thread_edit_totals SET added=added-OLD.edit_added,removed=removed-OLD.edit_removed,operations=operations-1 WHERE thread_id=OLD.thread_id;
       END;",
        ),
        Step::Rust(backfill_edit_totals),
    ],
    // Canonical engine settings and question identities. This was an SQL
    // no-op plus a Rust translation; the translation is the whole step.
    &[Step::Rust(migrate_payloads)],
    &[Step::Sql(
        r"UPDATE runs SET body=json_set(body,'$.input_cutoff',seq);
       CREATE INDEX items_pending_inputs ON items(thread_id,seq)
        WHERE status='running' AND json_extract(body,'$.kind.type')='user_message';",
    )],
    // Delivered inputs keep their original position while their revision advances.
    // A lost historical revision cannot be recovered from metadata updated_at.
    &[Step::Sql(
        r"UPDATE threads SET body=json_set(body,'$.activity_at',COALESCE(
       (SELECT json_extract(i.body,'$.updated_at') FROM items i
        WHERE i.thread_id=threads.id AND i.seq=json_extract(threads.body,'$.activity_seq')),
       (SELECT json_extract(i.body,'$.created_at') FROM items i
        WHERE i.thread_id=threads.id AND i.position=json_extract(threads.body,'$.activity_seq')
          AND json_extract(i.body,'$.kind.type')='user_message'),
       (SELECT COALESCE(json_extract(r.body,'$.ended_at'),json_extract(r.body,'$.started_at')) FROM runs r
        WHERE r.thread_id=threads.id AND r.seq<=json_extract(threads.body,'$.activity_seq')
        ORDER BY r.seq DESC LIMIT 1),
       json_extract(body,'$.created_at'),0));",
    )],
    // `item_content_delete` and `content_delete` are defined once, below.
    &[
        Step::Sql(
            r"CREATE TABLE content(id TEXT PRIMARY KEY, bytes INTEGER NOT NULL, lines INTEGER NOT NULL,
       diff_added INTEGER NOT NULL, diff_removed INTEGER NOT NULL, unreferenced_at INTEGER);
       CREATE INDEX content_orphans ON content(unreferenced_at) WHERE unreferenced_at IS NOT NULL;
       CREATE TABLE content_chunks(content_id TEXT NOT NULL,start INTEGER NOT NULL,data BLOB NOT NULL,PRIMARY KEY(content_id,start)) WITHOUT ROWID;
       CREATE TABLE item_content(item_id TEXT NOT NULL,content_id TEXT NOT NULL,PRIMARY KEY(item_id,content_id)) WITHOUT ROWID;
       CREATE INDEX item_content_source ON item_content(content_id);
       CREATE TRIGGER item_content_insert AFTER INSERT ON item_content BEGIN
         UPDATE content SET unreferenced_at=NULL WHERE id=NEW.content_id;
       END;
       CREATE TRIGGER item_content_owner_delete AFTER DELETE ON items BEGIN
         DELETE FROM item_content WHERE item_id=OLD.id;
       END;",
        ),
        Step::Rust(migrate_sources),
    ],
    &[Step::Sql(COLLECTIONS), Step::Rust(migrate_collections)],
    &[Step::Sql(SOURCE_INDEXES)],
    // Kind and parent are columns, so indexes and queries no longer have to
    // repeat the same `json_extract` text to match.
    &[Step::Sql(
        r"ALTER TABLE items ADD COLUMN kind_type TEXT;
       ALTER TABLE items ADD COLUMN parent_id TEXT;
       UPDATE items SET kind_type=json_extract(body,'$.kind.type'),parent_id=json_extract(body,'$.parent_id');
       DROP INDEX IF EXISTS items_pending_input;
       DROP INDEX IF EXISTS items_pending_inputs;
       DROP INDEX IF EXISTS items_thread_kind_position;
       DROP INDEX IF EXISTS items_thread_parent_position;
       DROP INDEX IF EXISTS items_agent_recipient_position;
       CREATE INDEX items_pending_input ON items(thread_id,position) WHERE status='running' AND kind_type='user_message';
       CREATE INDEX items_pending_inputs ON items(thread_id,seq) WHERE status='running' AND kind_type='user_message';
       CREATE INDEX items_thread_kind_position ON items(thread_id,kind_type,position);
       CREATE INDEX items_thread_parent_position ON items(thread_id,parent_id,position);
       CREATE INDEX items_agent_recipient_position ON items(thread_id,json_extract(body,'$.kind.tool.recipient'),position)
        WHERE parent_id IS NULL AND json_extract(body,'$.kind.tool.kind')='agent_message';",
    )],
    // Approvals live on the item they gate, and a message to an agent names
    // its Agent item instead of carrying display text.
    &[
        Step::Rust(migrate_approvals),
        Step::Sql(
            r"UPDATE items SET body=json_remove(body,'$.kind.tool.recipient_label')
         WHERE kind_type='tool' AND json_extract(body,'$.kind.tool.kind')='agent_message';
       UPDATE items SET body=json_set(body,'$.kind.tool.target',(SELECT a.id FROM items a
           WHERE a.thread_id=items.thread_id AND a.kind_type='agent'
             AND json_extract(a.body,'$.kind.agent_id')=json_extract(items.body,'$.kind.tool.recipient')
           ORDER BY a.position DESC LIMIT 1))
         WHERE kind_type='tool' AND json_extract(body,'$.kind.tool.kind')='agent_message'
           AND EXISTS(SELECT 1 FROM items a WHERE a.thread_id=items.thread_id AND a.kind_type='agent'
             AND json_extract(a.body,'$.kind.agent_id')=json_extract(items.body,'$.kind.tool.recipient'));
       DROP INDEX IF EXISTS items_agent_recipient_position;
       CREATE INDEX items_agent_message_target ON items(thread_id,json_extract(body,'$.kind.tool.target'),position)
        WHERE parent_id IS NULL AND json_extract(body,'$.kind.tool.kind')='agent_message';",
        ),
    ],
    // Claude repeats its `compacting` status as progress, and each repeat
    // used to start another compaction item. Keep one per compaction: the
    // one that has token counts, otherwise the first.
    &[Step::Sql(
        r"DELETE FROM items WHERE kind_type='compaction'
         AND json_extract(body,'$.kind.tokens_before') IS NULL
         AND EXISTS(SELECT 1 FROM items c WHERE c.run_id=items.run_id AND c.kind_type='compaction'
           AND (json_extract(c.body,'$.kind.tokens_before') IS NOT NULL OR c.position<items.position));",
    )],
    // Recall: every item's readable text under the item's rowid, ranked
    // search over it, and tool output kept apart (see `recall`). The index is
    // derived, so it is rebuilt whole from the items.
    &[
        Step::Sql(
            r"DROP TRIGGER IF EXISTS item_text_owner_delete;
       DROP TABLE IF EXISTS item_search;
       DROP TABLE IF EXISTS item_text;
       CREATE TABLE item_text(rowid INTEGER PRIMARY KEY, thread_id TEXT NOT NULL, item_id TEXT NOT NULL,
         kind TEXT NOT NULL, created_at INTEGER NOT NULL, text TEXT NOT NULL, output TEXT NOT NULL, paths TEXT NOT NULL);
       CREATE INDEX item_text_thread ON item_text(thread_id,rowid);
       CREATE VIRTUAL TABLE item_search USING fts5(text,paths,content='item_text',content_rowid='rowid',tokenize='trigram');
       CREATE TRIGGER item_text_owner_delete AFTER DELETE ON items BEGIN
         INSERT INTO item_search(item_search,rowid,text,paths) SELECT 'delete',rowid,text,paths FROM item_text WHERE rowid=OLD.rowid;
         DELETE FROM item_text WHERE rowid=OLD.rowid;
       END;",
        ),
        Step::Rust(crate::recall::backfill),
    ],
    // Calls to Artisan's own tools leave recall: a search found its own query.
    &[Step::Rust(crate::recall::forget_artisan_calls)],
    // Threads whose stopped turn should carry on with no new message; the
    // dispatcher starts them like threads with pending input.
    &[Step::Sql(
        "CREATE TABLE IF NOT EXISTS continues(thread_id TEXT PRIMARY KEY, seq INTEGER NOT NULL, reason TEXT NOT NULL);",
    )],
    // Transcript pages count the rows a reader sees and summarise the work
    // between them, so they need to know, by index, which items fold into
    // work (`Item::folds_into_work`).
    &[
        Step::Sql("ALTER TABLE items ADD COLUMN work INTEGER NOT NULL DEFAULT 0;"),
        Step::Rust(backfill_work),
        Step::Sql(
            "CREATE INDEX items_transcript_rows ON items(thread_id,position) WHERE parent_id IS NULL AND work=0;",
        ),
    ],
    // A failed turn waits in `continues` until `due_at` (ms) when Forge
    // picks it up again by itself (the provider was overloaded, or the
    // usage limit resets); requests without one go at once.
    &[Step::Sql(
        "ALTER TABLE continues ADD COLUMN due_at INTEGER;",
    )],
    // Each model comes back with the config last chosen for it; the old
    // Editor's entries still say access and approvals, which became one
    // choice (see the default config's step above).
    &[Step::Rust(migrate_model_permissions)],
];

/// Gives every remembered model config the permissions its old access and
/// approvals meant: what never asked stays Full, everything else is Auto.
fn migrate_model_permissions(conn: &Connection) -> Result<()> {
    use serde_json::Value;
    let Some(text) = conn
        .query_row(
            "SELECT value FROM kv WHERE key=?1",
            [artisan_api::MODEL_CONFIGS_KEY],
            |row| row.get::<_, String>(0),
        )
        .optional()?
    else {
        return Ok(());
    };
    let mut configs: Value = serde_json::from_str(&text).context("read the model configs")?;
    let Some(configs_map) = configs.as_object_mut() else {
        return Ok(());
    };
    for config in configs_map.values_mut() {
        let Some(config) = config.as_object_mut() else {
            continue;
        };
        if config.contains_key("permissions") {
            continue;
        }
        let never_asks = config.get("approvals").and_then(Value::as_str) == Some("never")
            || config
                .get("options")
                .and_then(|options| options.get("permission_mode"))
                .and_then(Value::as_str)
                == Some("bypassPermissions");
        config.insert(
            "permissions".into(),
            Value::String(if never_asks { "full" } else { "auto" }.into()),
        );
        config.remove("access");
        config.remove("approvals");
        if let Some(options) = config.get_mut("options").and_then(Value::as_object_mut) {
            options.remove("permission_mode");
            options.remove("mode");
        }
    }
    conn.execute(
        "UPDATE kv SET value=?2 WHERE key=?1",
        rusqlite::params![artisan_api::MODEL_CONFIGS_KEY, configs.to_string()],
    )?;
    Ok(())
}

/// Marks the root items that fold into work. An item whose body no longer
/// parses ships as a row, where the fold still places it.
fn backfill_work(conn: &Connection) -> Result<()> {
    let mut select = conn.prepare("SELECT id,body FROM items WHERE parent_id IS NULL")?;
    let mut rows = select.query([])?;
    let mut update = conn.prepare("UPDATE items SET work=1 WHERE id=?1")?;
    while let Some(row) = rows.next()? {
        let body: String = row.get(1)?;
        if serde_json::from_str::<artisan_api::Item>(&body).is_ok_and(|item| item.folds_into_work())
        {
            update.execute([row.get::<_, String>(0)?])?;
        }
    }
    Ok(())
}

/// An approval stored as its own item (`{"type":"approval", target,
/// approval}`, written by Codex and ACP runs before approvals moved onto the
/// item they gate) as a standalone tool that carries it. Migration 17 first
/// moves each one onto its target; this keeps the rest, and old databases
/// whose earlier migrations read every item, loadable.
pub fn legacy_approval(item: &mut serde_json::Value) {
    let Some(kind) = item.get_mut("kind") else {
        return;
    };
    if kind.get("type").and_then(serde_json::Value::as_str) != Some("approval") {
        return;
    }
    let approval = kind
        .get_mut("approval")
        .map(serde_json::Value::take)
        .unwrap_or_default();
    *kind = serde_json::json!({
        "type": "tool",
        "tool": {"kind": "other", "name": "approval", "input": null},
        "title": null,
        "output": null,
        "exit_code": null,
        "approval": approval,
    });
}

/// Moves each stored approval onto the tool or plan it gated, when that
/// item has none of its own, and drops the approval's own row; any other
/// approval becomes the standalone tool [`legacy_approval`] makes. Bodies
/// change in place: no seq, no event.
fn migrate_approvals(conn: &Connection) -> Result<()> {
    use serde_json::Value;
    let approvals: Vec<(String, String, String)> = conn
        .prepare(
            "SELECT id,thread_id,body FROM items WHERE kind_type='approval' ORDER BY position",
        )?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    for (id, thread, body) in approvals {
        let mut value: Value =
            serde_json::from_str(&body).with_context(|| format!("read approval {id}"))?;
        let target = value
            .pointer("/kind/target")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let gated: Option<(String, Value)> = match &target {
            Some(target) => conn
                .query_row(
                    "SELECT id,body FROM items WHERE id=?1 AND thread_id=?2
                     AND kind_type IN ('tool','plan') AND json_extract(body,'$.kind.approval') IS NULL",
                    rusqlite::params![target, thread],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()?
                .map(|(id, body)| serde_json::from_str(&body).map(|body| (id, body)))
                .transpose()
                .with_context(|| format!("read the item approval {id} gated"))?,
            None => None,
        };
        if let Some((target, mut gated)) = gated {
            gated["kind"]["approval"] = value["kind"]["approval"].take();
            let item: artisan_api::Item = serde_json::from_value(gated)
                .with_context(|| format!("move approval {id} onto item {target}"))?;
            conn.execute(
                "UPDATE items SET body=?2 WHERE id=?1",
                rusqlite::params![target, serde_json::to_string(&item)?],
            )?;
            crate::content::own(conn, &item)?;
            conn.execute("DELETE FROM items WHERE id=?1", [&id])?;
        } else {
            legacy_approval(&mut value);
            let item: artisan_api::Item = serde_json::from_value(value)
                .with_context(|| format!("keep approval {id} as a tool"))?;
            conn.execute(
                "UPDATE items SET body=?2,kind_type='tool' WHERE id=?1",
                rusqlite::params![id, serde_json::to_string(&item)?],
            )?;
        }
    }
    Ok(())
}

/// Collection records with their dependency graph. A source stays while an
/// item or a parent source references it; `content_delete` cascades.
const COLLECTIONS: &str = r"
CREATE TABLE content_collections(content_id TEXT PRIMARY KEY, count INTEGER NOT NULL, authored_bytes INTEGER NOT NULL, structural_bytes INTEGER NOT NULL);
CREATE TABLE content_records(content_id TEXT NOT NULL, ordinal INTEGER NOT NULL,
  start INTEGER NOT NULL, end INTEGER NOT NULL, record_key TEXT, record_label TEXT, PRIMARY KEY(content_id,ordinal)) WITHOUT ROWID;
CREATE UNIQUE INDEX content_records_key ON content_records(content_id,record_key) WHERE record_key IS NOT NULL;
CREATE INDEX content_records_label ON content_records(content_id,record_label) WHERE record_label IS NOT NULL;
CREATE TABLE content_dependencies(parent_id TEXT NOT NULL,child_id TEXT NOT NULL,
  PRIMARY KEY(parent_id,child_id), CHECK(parent_id<>child_id)) WITHOUT ROWID;
CREATE INDEX content_dependencies_child ON content_dependencies(child_id);
CREATE TRIGGER content_dependency_insert AFTER INSERT ON content_dependencies BEGIN
  UPDATE content SET unreferenced_at=NULL WHERE id=NEW.child_id;
END;
CREATE TRIGGER content_dependency_delete AFTER DELETE ON content_dependencies BEGIN
  UPDATE content SET unreferenced_at=CAST(strftime('%s','now') AS INTEGER)*1000 WHERE id=OLD.child_id
    AND NOT EXISTS(SELECT 1 FROM item_content WHERE content_id=OLD.child_id)
    AND NOT EXISTS(SELECT 1 FROM content_dependencies WHERE child_id=OLD.child_id);
END;
-- Databases that applied an earlier migration 13 have item-only versions.
DROP TRIGGER IF EXISTS item_content_delete;
CREATE TRIGGER item_content_delete AFTER DELETE ON item_content BEGIN
  UPDATE content SET unreferenced_at=CAST(strftime('%s','now') AS INTEGER)*1000 WHERE id=OLD.content_id
    AND NOT EXISTS(SELECT 1 FROM item_content WHERE content_id=OLD.content_id)
    AND NOT EXISTS(SELECT 1 FROM content_dependencies WHERE child_id=OLD.content_id);
END;
DROP TRIGGER IF EXISTS content_delete;
CREATE TRIGGER content_delete AFTER DELETE ON content BEGIN
  DELETE FROM content_chunks WHERE content_id=OLD.id;
  DELETE FROM content_records WHERE content_id=OLD.id;
  DELETE FROM content_collections WHERE content_id=OLD.id;
  DELETE FROM content_dependencies WHERE parent_id=OLD.id;
END;
";

/// Persisted Markdown leaf indexes, removed with their source.
const SOURCE_INDEXES: &str = r"
CREATE TABLE source_indexes(source_id TEXT NOT NULL, format TEXT NOT NULL CHECK(format IN ('literal','markdown')), version INTEGER NOT NULL,
 metadata_id TEXT NOT NULL, leaf_count INTEGER NOT NULL, PRIMARY KEY(source_id,format,version)) WITHOUT ROWID;
CREATE TABLE source_index_leaves(source_id TEXT NOT NULL, format TEXT NOT NULL CHECK(format IN ('literal','markdown')), version INTEGER NOT NULL, ordinal INTEGER NOT NULL,
 source_start INTEGER NOT NULL, source_end INTEGER NOT NULL, metadata_start INTEGER NOT NULL, metadata_end INTEGER NOT NULL,
 PRIMARY KEY(source_id,format,version,ordinal)) WITHOUT ROWID;
CREATE TRIGGER source_index_delete AFTER DELETE ON content BEGIN
 DELETE FROM source_index_leaves WHERE source_id=OLD.id;
 DELETE FROM source_indexes WHERE source_id=OLD.id;
END;
";

pub fn migrate(conn: &mut Connection) -> Result<()> {
    migrate_to(conn, MIGRATIONS.len())
}

fn migrate_to(conn: &mut Connection, target: usize) -> Result<()> {
    let version: usize = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .context("read schema version")?;
    if version > MIGRATIONS.len() {
        bail!(
            "database schema version {version} is newer than this Forge ({})",
            MIGRATIONS.len()
        );
    }
    for (index, steps) in MIGRATIONS.iter().enumerate().take(target).skip(version) {
        let next = index + 1;
        tracing::info!("upgrading the database to schema {next} of {target}");
        let tx = conn.transaction()?;
        for step in *steps {
            match step {
                Step::Sql(sql) => tx.execute_batch(sql).map_err(Into::into),
                Step::Rust(translate) => translate(&tx),
            }
            .with_context(|| format!("apply schema migration {next}"))?;
        }
        tx.pragma_update(None, "user_version", next)?;
        tx.commit()
            .with_context(|| format!("commit schema migration {next}"))?;
    }
    Ok(())
}

/// Existing edit history is counted once, one row at a time, without
/// manufacturing Item revisions or stream events.
fn backfill_edit_totals(conn: &Connection) -> Result<()> {
    let mut select = conn.prepare(
        "SELECT id,body FROM items WHERE status='done' AND json_extract(body,'$.kind.tool.kind')='edit'",
    )?;
    let mut rows = select.query([])?;
    let mut update = conn.prepare("UPDATE items SET edit_added=?2,edit_removed=?3 WHERE id=?1")?;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let body: serde_json::Value = serde_json::from_str(&row.get::<_, String>(1)?)?;
        if let Some(tool) = body.pointer("/kind/tool") {
            let tool: artisan_api::ToolKind = serde_json::from_value(tool.clone())?;
            if let Some(stats) = crate::content::inline_edit_totals(&tool) {
                update.execute(rusqlite::params![id, stats.added, stats.removed])?;
            }
        }
    }
    Ok(())
}

fn migrate_sources(conn: &Connection) -> Result<()> {
    crate::content::migrate(conn, false)
}

fn migrate_collections(conn: &Connection) -> Result<()> {
    crate::content::migrate(conn, true)
}

/// One-time persisted-data translation, never a runtime provider fallback.
fn canonical_config(config: &mut serde_json::Value) {
    use serde_json::Value;
    let engine = config
        .get("engine")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let has = |value: &Value, key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty())
    };
    match engine.as_str() {
        "codex" => {
            let old = config
                .get_mut("options")
                .and_then(Value::as_object_mut)
                .and_then(|options| options.remove("service_tier"));
            if !has(config, "speed")
                && let Some(Value::String(speed)) = old
                && !speed.is_empty()
            {
                config["speed"] = Value::String(speed);
            }
        }
        "cursor" => {
            let old = config
                .get_mut("options")
                .and_then(Value::as_object_mut)
                .and_then(|options| options.remove("fast"));
            if !has(config, "speed") && old == Some(Value::Bool(true)) {
                config["speed"] = Value::String("fast".into());
            }
        }
        "opencode" => {
            let old = config
                .as_object_mut()
                .and_then(|config| config.remove("effort"));
            if !has(config, "variant")
                && let Some(Value::String(variant)) = old
                && !variant.is_empty()
            {
                config["variant"] = Value::String(variant);
            }
        }
        _ => {}
    }
}
fn canonical_question(body: &mut serde_json::Value) -> Result<()> {
    use serde_json::{Value, json};
    let kind = body.get_mut("kind").context("question has no kind")?;
    let questions = kind
        .get_mut("questions")
        .and_then(Value::as_array_mut)
        .context("question has no questions")?;
    for question in questions.iter_mut() {
        for (index, option) in question
            .get_mut("options")
            .and_then(Value::as_array_mut)
            .context("question options are not an array")?
            .iter_mut()
            .enumerate()
        {
            if option.get("id").is_none() {
                option["id"] = Value::String(format!("option-{index}"));
            }
        }
    }
    // Already-canonical answers remain untouched. Old label arrays cannot
    // recover duplicate-choice identity; preserve their first-match behavior once.
    let questions = questions.clone();
    if let Some(answers) = kind.get_mut("answers").and_then(Value::as_array_mut) {
        for (question, answer) in questions.iter().zip(answers) {
            let Some(old) = answer.as_array() else {
                continue;
            };
            if old.is_empty() {
                *answer = Value::Null;
                continue;
            }
            let labels: Vec<_> = old
                .iter()
                .map(|value| value.as_str().context("old question answer is not text"))
                .collect::<Result<_>>()?;
            let options = question["options"]
                .as_array()
                .context("question options are not an array")?;
            let ids: Option<Vec<_>> = labels
                .iter()
                .map(|label| {
                    options
                        .iter()
                        .find(|option| option["label"].as_str() == Some(*label))
                        .map(|option| option["id"].clone())
                })
                .collect();
            *answer = ids.map_or_else(
                || json!({"kind":"text","text":labels.join(", ")}),
                |ids| json!({"kind":"choices","ids":ids}),
            );
        }
    }
    Ok(())
}
fn migrate_payloads(conn: &Connection) -> Result<()> {
    let mut select = conn.prepare("SELECT id,body FROM threads")?;
    let mut rows = select.query([])?;
    let mut update = conn.prepare("UPDATE threads SET body=?2 WHERE id=?1")?;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let text: String = row.get(1)?;
        let mut value: serde_json::Value = serde_json::from_str(&text)?;
        if let Some(config) = value.get_mut("config") {
            canonical_config(config);
        }
        update.execute(rusqlite::params![id, value.to_string()])?;
    }
    for key in ["default_engine_config", "model_engine_configs"] {
        let value: Option<String> = conn
            .query_row("SELECT value FROM kv WHERE key=?1", [key], |row| row.get(0))
            .optional()?;
        if let Some(value) = value {
            let mut value: serde_json::Value = serde_json::from_str(&value)?;
            if key == "model_engine_configs" {
                if let Some(configs) = value.as_object_mut() {
                    for config in configs.values_mut() {
                        canonical_config(config);
                    }
                }
            } else {
                canonical_config(&mut value);
            }
            conn.execute(
                "UPDATE kv SET value=?2 WHERE key=?1",
                rusqlite::params![key, value.to_string()],
            )?;
        }
    }
    let mut select = conn
        .prepare("SELECT id,body FROM items WHERE json_extract(body,'$.kind.type')='question'")?;
    let mut rows = select.query([])?;
    let mut update = conn.prepare("UPDATE items SET body=?2 WHERE id=?1")?;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let mut value: serde_json::Value = serde_json::from_str(&row.get::<_, String>(1)?)?;
        canonical_question(&mut value)?;
        update.execute(rusqlite::params![id, value.to_string()])?;
    }
    Ok(())
}

#[cfg(test)]
mod canonical_migration_tests {
    use super::*;
    use serde_json::{Value, json};
    #[test]
    fn persisted_settings_and_old_question_answers_upgrade_once_without_new_revisions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.sqlite");
        let mut conn = Connection::open(&path).unwrap();
        migrate_to(&mut conn, 9).unwrap();
        let codex = json!({"engine":"codex","model":null,"effort":null,"options":{"service_tier":"priority","unrelated":42}});
        let cursor = json!({"engine":"cursor","model":null,"effort":null,"options":{"fast":true}});
        let opencode = json!({"engine":"opencode","model":null,"effort":"high","variant":"custom","options":{}});
        conn.execute(
            "INSERT INTO threads VALUES ('t',17,?1)",
            [json!({"config":codex}).to_string()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO kv VALUES ('default_engine_config',18,?1)",
            [cursor.to_string()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO kv VALUES ('model_engine_configs',19,?1)",
            [json!({"selected":opencode}).to_string()],
        )
        .unwrap();
        let question = json!({"id":"q","thread_id":"t","position":1,"seq":20,"run_id":null,"parent_id":null,"status":"done","created_at":1,"updated_at":1,"kind":{"type":"question","blocking":false,"questions":[{"id":"q","header":null,"text":"Pick","multi_select":false,"options":[{"label":"Same","description":null},{"label":"Same","description":null}]}],"answers":[["Same"]]}});
        conn.execute("INSERT INTO items (id,thread_id,position,seq,status,body) VALUES ('q','t',1,20,'done',?1)",[question.to_string()]).unwrap();
        migrate(&mut conn).unwrap();
        drop(conn);
        let mut conn = Connection::open(&path).unwrap();
        migrate(&mut conn).unwrap();
        let body: String = conn
            .query_row("SELECT body FROM threads WHERE id='t'", [], |row| {
                row.get(0)
            })
            .unwrap();
        let body: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(body["config"]["speed"], "priority");
        assert_eq!(body["config"]["options"], json!({"unrelated":42}));
        let settings: Vec<(String, i64, String)> = conn
            .prepare("SELECT key,seq,value FROM kv ORDER BY key")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(settings[0].1, 18);
        assert_eq!(settings[1].1, 19);
        // Cursor is retired; its old config is still translated, as JSON.
        let default: Value = serde_json::from_str(&settings[0].2).unwrap();
        assert_eq!(default["speed"], "fast");
        assert!(default["options"].get("fast").is_none());
        let models: Value = serde_json::from_str(&settings[1].2).unwrap();
        assert_eq!(models["selected"]["variant"], "custom");
        assert!(models["selected"].get("effort").is_none());
        let (seq, body): (i64, String) = conn
            .query_row("SELECT seq,body FROM items WHERE id='q'", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        assert_eq!(seq, 20);
        let body: Value = serde_json::from_str(&body).unwrap();
        let kind: artisan_api::ItemKind = serde_json::from_value(body["kind"].clone()).unwrap();
        let artisan_api::ItemKind::Question {
            questions, answers, ..
        } = kind
        else {
            panic!("question changed type")
        };
        let options = questions.inline().unwrap()[0].options.inline().unwrap();
        assert_ne!(options[0].id, options[1].id);
        assert_eq!(
            answers,
            Some(
                vec![Some(artisan_api::QuestionAnswer::Choices {
                    ids: vec!["option-0".into()].into()
                })]
                .into()
            )
        );
        assert_eq!(
            conn.query_row("SELECT seq FROM threads WHERE id='t'", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            17
        );
    }

    /// The old Editor's per-model choices keep their access: what never
    /// asked comes back as Full access, the rest as Auto.
    #[test]
    fn remembered_model_configs_keep_full_access() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate_to(&mut conn, MIGRATIONS.len() - 1).unwrap();
        let configs = json!({
            "claude-opus-5-5": {"engine":"claude","model":"claude-opus-5-5","effort":"medium",
                "access":"full","approvals":"never","options":{"permission_mode":"bypassPermissions"}},
            "claude-sonnet-5-5": {"engine":"claude","model":"claude-sonnet-5-5","effort":"medium",
                "access":"full","approvals":"on_request","options":{"permission_mode":"auto"}},
        });
        conn.execute(
            "INSERT INTO kv VALUES ('model_engine_configs',7,?1)",
            [configs.to_string()],
        )
        .unwrap();
        migrate(&mut conn).unwrap();
        let value: String = conn
            .query_row(
                "SELECT value FROM kv WHERE key='model_engine_configs'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let value: Value = serde_json::from_str(&value).unwrap();
        let config = |model: &str| {
            artisan_api::saved_model_config(&value, artisan_api::EngineId::Claude, model).unwrap()
        };
        assert_eq!(
            config("claude-opus-5-5").permissions,
            artisan_api::Permissions::Full
        );
        assert_eq!(
            config("claude-sonnet-5-5").permissions,
            artisan_api::Permissions::Auto
        );
        assert!(config("claude-opus-5-5").options.is_empty());
    }
}

#[cfg(test)]
mod activity_time_migration_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn activity_backfill_uses_source_events_and_preserves_revisions() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate_to(&mut conn, 11).unwrap();
        for (id, activity) in [
            ("edited-input", 20),
            ("delivered-input", 30),
            ("run", 40),
            ("older-run", 60),
            ("missing-event", 70),
        ] {
            conn.execute(
                "INSERT INTO threads VALUES (?1,99,?2)",
                rusqlite::params![
                    id,
                    json!({"activity_seq":activity,"created_at":10,"updated_at":9999}).to_string()
                ],
            )
            .unwrap();
        }
        conn.execute("INSERT INTO items(id,thread_id,position,seq,status,body) VALUES('a','edited-input',15,20,'done',?1)",
            [json!({"id":"a","thread_id":"edited-input","position":15,"seq":20,"run_id":null,"parent_id":null,"status":"done","created_at":100,"updated_at":200,"kind":{"type":"user_message","text":"edited input","images":[],"steer":false}}).to_string()]).unwrap();
        conn.execute("INSERT INTO items(id,thread_id,position,seq,status,body) VALUES('b','delivered-input',30,31,'done',?1)",
            [json!({"id":"b","thread_id":"delivered-input","position":30,"seq":31,"run_id":null,"parent_id":null,"status":"done","created_at":300,"updated_at":310,"kind":{"type":"user_message","text":"delivered input","images":[],"steer":false}}).to_string()]).unwrap();
        conn.execute(
            "INSERT INTO runs VALUES('r','run',40,'done',?1)",
            [json!({"started_at":350,"ended_at":400}).to_string()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO runs VALUES('older','older-run',50,'done',?1)",
            [json!({"started_at":450,"ended_at":500}).to_string()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO runs VALUES('newer','older-run',65,'done',?1)",
            [json!({"started_at":600,"ended_at":650}).to_string()],
        )
        .unwrap();
        migrate(&mut conn).unwrap();
        for (id, expected) in [
            ("edited-input", 200),
            ("delivered-input", 300),
            ("run", 400),
            ("older-run", 500),
            ("missing-event", 10),
        ] {
            let (seq, activity_at, updated_at): (u64, i64, i64) = conn.query_row(
                "SELECT seq,json_extract(body,'$.activity_at'),json_extract(body,'$.updated_at') FROM threads WHERE id=?1",
                [id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).unwrap();
            assert_eq!((seq, activity_at, updated_at), (99, expected, 9999), "{id}");
        }
        migrate(&mut conn).unwrap();
        assert_eq!(
            conn.pragma_query_value::<usize, _>(None, "user_version", |row| row.get(0))
                .unwrap(),
            MIGRATIONS.len()
        );
    }
}

#[cfg(test)]
mod item_column_migration_tests {
    use super::*;
    use artisan_api::{Item, ItemId, ItemKind, ItemsQuery, Status, ThreadId};

    const WHOLE: ItemsQuery = ItemsQuery {
        before: None,
        after: None,
        limit: None,
    };

    fn item(id: &str, parent: Option<&str>, status: Status, kind: ItemKind) -> Item {
        Item {
            id: ItemId(id.into()),
            thread_id: ThreadId("t".into()),
            position: 0,
            seq: 0,
            run_id: None,
            parent_id: parent.map(|parent| ItemId(parent.into())),
            status,
            created_at: 1,
            updated_at: 1,
            kind,
        }
    }

    /// Items stored before kind and parent became columns still start runs
    /// and still show in their agent's transcript.
    #[test]
    fn existing_items_keep_their_queue_and_transcript_after_the_column_migration() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate_to(&mut conn, 15).unwrap();
        let rows = [
            item(
                "input",
                None,
                Status::Running,
                ItemKind::UserMessage {
                    text: "pending".into(),
                    images: Vec::new(),
                    steer: false,
                    from_agent: None,
                    sender: None,
                },
            ),
            item(
                "child",
                Some("agent"),
                Status::Done,
                ItemKind::Notice {
                    kind: artisan_api::NoticeKind::Info,
                    text: "subagent work".into(),
                },
            ),
        ];
        for (seq, row) in rows.iter().enumerate() {
            conn.execute(
                "INSERT INTO items(id,thread_id,position,seq,status,body) VALUES(?1,'t',?2,?2,?3,?4)",
                rusqlite::params![
                    row.id.as_str(),
                    seq + 1,
                    crate::read::tag(&row.status).unwrap(),
                    serde_json::to_string(row).unwrap()
                ],
            )
            .unwrap();
        }
        migrate(&mut conn).unwrap();
        assert_eq!(
            crate::read::pending_threads(&conn, 8, &[]).unwrap(),
            [ThreadId("t".into())]
        );
        let agent = ItemId("agent".into());
        let page = crate::read::scoped_items_page(
            &conn,
            &ThreadId("t".into()),
            &WHOLE,
            Some(Some(&agent)),
        )
        .unwrap();
        assert_eq!(
            page.items
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["child"]
        );
        let root = crate::read::scoped_items_page(&conn, &ThreadId("t".into()), &WHOLE, Some(None))
            .unwrap();
        assert_eq!(
            root.items
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["input"]
        );
    }

    /// A Claude compaction stored once per progress frame shows once: the
    /// thread keeps the item with token counts and its other compactions.
    #[test]
    fn repeated_compaction_items_collapse_to_one_per_compaction() {
        use serde_json::json;
        let mut conn = Connection::open_in_memory().unwrap();
        migrate_to(&mut conn, 17).unwrap();
        let compaction = |id: &str, run: &str, position: i64, before: Option<u64>| {
            let body = json!({"id":id,"thread_id":"t","position":position,"seq":position,
                "run_id":run,"parent_id":null,"status":"done","created_at":1,"updated_at":1,
                "kind":{"type":"compaction","tokens_before":before,
                    "tokens_after":before.map(|_| 15_000)}});
            conn.execute(
                "INSERT INTO items(id,thread_id,position,seq,run_id,status,body,kind_type)
                 VALUES(?1,'t',?2,?2,?3,'done',?4,'compaction')",
                rusqlite::params![id, position, run, body.to_string()],
            )
            .unwrap();
        };
        compaction("progress-1", "r1", 1, None);
        compaction("progress-2", "r1", 2, None);
        compaction("boundary", "r1", 3, Some(966_000));
        compaction("interrupted-1", "r2", 4, None);
        compaction("interrupted-2", "r2", 5, None);
        compaction("other-run", "r3", 6, Some(500_000));
        migrate(&mut conn).unwrap();
        let page =
            crate::read::scoped_items_page(&conn, &ThreadId("t".into()), &WHOLE, None).unwrap();
        let ids: Vec<&str> = page.items.iter().map(|item| item.id.as_str()).collect();
        assert_eq!(ids, ["boundary", "interrupted-1", "other-run"]);
    }

    /// Threads Codex and ACP ran before approvals moved onto the item they
    /// gate still load: each approval shows on its command, and a message to
    /// an agent still shows in that agent's transcript.
    #[test]
    fn stored_approvals_move_onto_their_items_and_agent_messages_name_their_agent() {
        use serde_json::json;
        let mut conn = Connection::open_in_memory().unwrap();
        migrate_to(&mut conn, 16).unwrap();
        let row = |id: &str, position: i64, status: &str, kind: serde_json::Value| {
            json!({"id":id,"thread_id":"t","position":position,"seq":position,"run_id":null,
                "parent_id":null,"status":status,"created_at":1,"updated_at":1,"kind":kind})
        };
        let approval = json!({"request":{"type":"command","command":"touch a","cwd":"/w"},
            "reason":null,"options":["allow_once","deny_once"],"decision":"allow_once"});
        let rows = [
            row(
                "tool",
                1,
                "done",
                json!({"type":"tool","tool":{"kind":"shell","command":"touch a","cwd":"/w"},
                "title":null,"output":null,"exit_code":0,"approval":null}),
            ),
            row(
                "asked",
                2,
                "done",
                json!({"type":"approval","target":"tool","approval":approval}),
            ),
            row(
                "orphan",
                3,
                "declined",
                json!({"type":"approval","target":null,"approval":approval}),
            ),
            row(
                "agent",
                4,
                "running",
                json!({"type":"agent","name":null,"description":"task",
                "agent_id":"a935b57d5f0b8fb33","background":true,"result":null}),
            ),
            row(
                "sent",
                5,
                "done",
                json!({"type":"tool","tool":{"kind":"agent_message",
                "recipient":"a935b57d5f0b8fb33","recipient_label":"an agent","summary":null,"text":"hi"},
                "title":null,"output":null,"exit_code":null,"approval":null}),
            ),
        ];
        for row in &rows {
            conn.execute(
                "INSERT INTO items(id,thread_id,position,seq,status,body,kind_type) VALUES(?1,'t',?2,?2,?3,?4,json_extract(?4,'$.kind.type'))",
                rusqlite::params![
                    row["id"].as_str().unwrap(),
                    row["position"].as_i64().unwrap(),
                    row["status"].as_str().unwrap(),
                    row.to_string()
                ],
            )
            .unwrap();
        }
        migrate(&mut conn).unwrap();
        let page =
            crate::read::scoped_items_page(&conn, &ThreadId("t".into()), &WHOLE, None).unwrap();
        let ids: Vec<&str> = page.items.iter().map(|item| item.id.as_str()).collect();
        assert_eq!(ids, ["tool", "orphan", "agent", "sent"]);
        let ItemKind::Tool {
            approval: Some(moved),
            ..
        } = &page.items[0].kind
        else {
            panic!("the approval moved onto its command: {:?}", page.items[0]);
        };
        assert_eq!(moved.decision, Some(artisan_api::Decision::AllowOnce));
        assert!(matches!(
            &page.items[1].kind,
            ItemKind::Tool {
                approval: Some(_),
                ..
            }
        ));
        let agent = ItemId("agent".into());
        let scoped = crate::read::scoped_items_page(
            &conn,
            &ThreadId("t".into()),
            &WHOLE,
            Some(Some(&agent)),
        )
        .unwrap();
        assert_eq!(scoped.items.len(), 1);
        assert!(matches!(
            &scoped.items[0].kind,
            ItemKind::Tool {
                tool: artisan_api::ToolKind::AgentMessage { target: Some(target), .. },
                ..
            } if *target == agent
        ));
    }
}
