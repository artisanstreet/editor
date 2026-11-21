CREATE TABLE content(id TEXT PRIMARY KEY, bytes INTEGER NOT NULL, lines INTEGER NOT NULL,
diff_added INTEGER NOT NULL DEFAULT 0, diff_removed INTEGER NOT NULL DEFAULT 0, unreferenced_at INTEGER);
CREATE INDEX content_orphans ON content(unreferenced_at) WHERE unreferenced_at IS NOT NULL;
CREATE TABLE content_chunks(content_id TEXT NOT NULL,start INTEGER NOT NULL,data BLOB NOT NULL,PRIMARY KEY(content_id,start)) WITHOUT ROWID;
CREATE TABLE item_content(item_id TEXT NOT NULL,content_id TEXT NOT NULL,PRIMARY KEY(item_id,content_id)) WITHOUT ROWID;
CREATE INDEX item_content_source ON item_content(content_id);
CREATE TRIGGER item_content_insert AFTER INSERT ON item_content BEGIN
  UPDATE content SET unreferenced_at=NULL WHERE id=NEW.content_id;
END;
CREATE TRIGGER item_content_owner_delete AFTER DELETE ON items BEGIN
  DELETE FROM item_content WHERE item_id=OLD.id;
END;
