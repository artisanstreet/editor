CREATE TABLE work_spans(thread_id TEXT NOT NULL, first INTEGER NOT NULL, last INTEGER NOT NULL, run_id TEXT,
  items INTEGER NOT NULL CHECK(items > 0), worked INTEGER NOT NULL CHECK(worked BETWEEN 0 AND items),
  PRIMARY KEY(thread_id, first)) WITHOUT ROWID;
CREATE UNIQUE INDEX work_spans_last ON work_spans(thread_id, last);
