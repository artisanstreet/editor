use anyhow::{Context, Result, bail};
use rusqlite::Connection;

/// Applied in order; `PRAGMA user_version` counts how many have run.
/// Never edit a released entry, append a new one.
const MIGRATIONS: &[&str] = &[
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
    r"
-- Values only Forge reads: never in /state or the event stream.
CREATE TABLE secrets (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
INSERT INTO secrets (key, value) SELECT key, value FROM kv WHERE key = 'forge_identity';
DELETE FROM kv WHERE key = 'forge_identity';
",
    r"
-- Every saved version of the global prompt; the latest is also the
-- `global_prompt` setting.
CREATE TABLE prompt_versions (
    version  INTEGER PRIMARY KEY,
    text     TEXT NOT NULL,
    saved_at INTEGER NOT NULL
);
",
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
];

pub(crate) fn migrate(conn: &mut Connection) -> Result<()> {
    let version: usize = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .context("read schema version")?;
    if version > MIGRATIONS.len() {
        bail!(
            "database schema version {version} is newer than this Forge ({})",
            MIGRATIONS.len()
        );
    }
    for (index, sql) in MIGRATIONS.iter().enumerate().skip(version) {
        let next = index + 1;
        let tx = conn.transaction()?;
        tx.execute_batch(sql)
            .with_context(|| format!("apply schema migration {next}"))?;
        tx.pragma_update(None, "user_version", next)?;
        tx.commit()
            .with_context(|| format!("commit schema migration {next}"))?;
    }
    Ok(())
}
