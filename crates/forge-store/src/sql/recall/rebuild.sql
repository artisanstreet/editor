DROP TRIGGER IF EXISTS item_text_owner_delete;
DROP TABLE IF EXISTS message_search;
DROP TABLE IF EXISTS item_search;
DROP TABLE IF EXISTS item_text;
DROP TABLE IF EXISTS recall_index;
CREATE TABLE item_text(rowid INTEGER PRIMARY KEY, thread_id TEXT NOT NULL, item_id TEXT NOT NULL,
  kind TEXT NOT NULL, created_at INTEGER NOT NULL, text TEXT NOT NULL, output TEXT NOT NULL, paths TEXT NOT NULL,
  folded TEXT NOT NULL DEFAULT '');
CREATE INDEX item_text_thread ON item_text(thread_id,rowid);
CREATE VIRTUAL TABLE item_search USING fts5(text,paths,content='item_text',content_rowid='rowid',tokenize='trigram');
CREATE VIRTUAL TABLE message_search USING fts5(folded,content='item_text',content_rowid='rowid',tokenize='trigram case_sensitive 1',columnsize=0);
CREATE TRIGGER item_text_owner_delete AFTER DELETE ON items BEGIN
  INSERT INTO item_search(item_search,rowid,text,paths) SELECT 'delete',rowid,text,paths FROM item_text WHERE rowid=OLD.rowid;
  INSERT INTO message_search(message_search,rowid,folded) SELECT 'delete',rowid,folded FROM item_text WHERE rowid=OLD.rowid;
  DELETE FROM item_text WHERE rowid=OLD.rowid;
END;
CREATE TABLE recall_index(version INTEGER NOT NULL, building_after INTEGER);
