CREATE TABLE IF NOT EXISTS removed_threads(id TEXT PRIMARY KEY, seq INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS removed_threads_seq ON removed_threads(seq);
