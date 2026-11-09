CREATE TABLE coordination_records (
    id TEXT PRIMARY KEY,
    owner TEXT NOT NULL,
    project_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    revision INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    body TEXT NOT NULL
);
CREATE INDEX coordination_project_kind ON coordination_records(project_id,kind,updated_at DESC,id);
CREATE INDEX coordination_owner ON coordination_records(owner,kind);
CREATE TABLE coordination_events (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    record_id TEXT NOT NULL,
    owner TEXT NOT NULL,
    project_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    revision INTEGER NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX coordination_events_project ON coordination_events(project_id,seq);
