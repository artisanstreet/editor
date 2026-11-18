DROP TABLE IF EXISTS message_search;
CREATE VIRTUAL TABLE message_search USING fts5(folded,content='item_text',content_rowid='rowid',tokenize='trigram case_sensitive 1',columnsize=0);
DROP TRIGGER IF EXISTS item_text_owner_delete;
CREATE TRIGGER item_text_owner_delete AFTER DELETE ON items BEGIN
  INSERT INTO item_search(item_search,rowid,text,paths) SELECT 'delete',rowid,text,paths FROM item_text WHERE rowid=OLD.rowid;
  INSERT INTO message_search(message_search,rowid,folded) SELECT 'delete',rowid,folded FROM item_text WHERE rowid=OLD.rowid;
  DELETE FROM item_text WHERE rowid=OLD.rowid;
END;
