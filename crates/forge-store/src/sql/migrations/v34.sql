CREATE TABLE outline_revisions(thread_id TEXT PRIMARY KEY, revision INTEGER NOT NULL) WITHOUT ROWID;
INSERT INTO outline_revisions(thread_id, revision)
  SELECT thread_id, MAX(seq) FROM items WHERE outline IS NOT NULL GROUP BY thread_id;
CREATE TRIGGER outline_revision_insert AFTER INSERT ON items WHEN NEW.outline IS NOT NULL BEGIN
  INSERT INTO outline_revisions(thread_id, revision) VALUES(NEW.thread_id, NEW.seq)
    ON CONFLICT(thread_id) DO UPDATE SET revision=excluded.revision;
END;
CREATE TRIGGER outline_revision_update AFTER UPDATE OF outline ON items WHEN OLD.outline IS NOT NEW.outline BEGIN
  INSERT INTO outline_revisions(thread_id, revision) VALUES(NEW.thread_id, NEW.seq)
    ON CONFLICT(thread_id) DO UPDATE SET revision=excluded.revision;
END;
CREATE INDEX items_live_by_thread ON items(thread_id,position) WHERE status IN ('running','awaiting_user');
