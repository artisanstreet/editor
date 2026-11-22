CREATE TABLE content_diffs(content_id TEXT PRIMARY KEY, added INTEGER NOT NULL, removed INTEGER NOT NULL) WITHOUT ROWID;
CREATE TRIGGER content_diff_delete AFTER DELETE ON content BEGIN
  DELETE FROM content_diffs WHERE content_id=OLD.id;
END;
ALTER TABLE content DROP COLUMN diff_added;
ALTER TABLE content DROP COLUMN diff_removed;
