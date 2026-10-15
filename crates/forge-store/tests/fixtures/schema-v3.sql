-- Released schema3 fixture, deliberately independent of later migrations.

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


-- Values only Forge reads: never in /state or the event stream.
CREATE TABLE secrets (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
INSERT INTO secrets (key, value) SELECT key, value FROM kv WHERE key = 'forge_identity';
DELETE FROM kv WHERE key = 'forge_identity';


-- Every saved version of the global prompt; the latest is also the
-- `global_prompt` setting.
CREATE TABLE prompt_versions (
    version  INTEGER PRIMARY KEY,
    text     TEXT NOT NULL,
    saved_at INTEGER NOT NULL
);

PRAGMA user_version = 3;
