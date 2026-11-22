DROP TRIGGER IF EXISTS item_text_owner_delete;
DROP TABLE IF EXISTS item_search;
DROP TABLE IF EXISTS item_text;
CREATE TABLE item_text(rowid INTEGER PRIMARY KEY, thread_id TEXT NOT NULL, item_id TEXT NOT NULL,
  kind TEXT NOT NULL, created_at INTEGER NOT NULL, text TEXT NOT NULL, output TEXT NOT NULL, paths TEXT NOT NULL);
CREATE INDEX item_text_thread ON item_text(thread_id,rowid);
CREATE VIRTUAL TABLE item_search USING fts5(text,paths,content='item_text',content_rowid='rowid',tokenize='trigram');
CREATE TRIGGER item_text_owner_delete AFTER DELETE ON items BEGIN
  INSERT INTO item_search(item_search,rowid,text,paths) SELECT 'delete',rowid,text,paths FROM item_text WHERE rowid=OLD.rowid;
  DELETE FROM item_text WHERE rowid=OLD.rowid;
END;
