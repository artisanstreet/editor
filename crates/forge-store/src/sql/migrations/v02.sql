CREATE TABLE secrets (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
INSERT INTO secrets (key, value) SELECT key, value FROM kv WHERE key = 'forge_identity';
DELETE FROM kv WHERE key = 'forge_identity';
