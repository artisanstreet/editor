CREATE TABLE content_collections(content_id TEXT PRIMARY KEY, count INTEGER NOT NULL, authored_bytes INTEGER NOT NULL, structural_bytes INTEGER NOT NULL);
CREATE TABLE content_records(content_id TEXT NOT NULL, ordinal INTEGER NOT NULL,
  start INTEGER NOT NULL, end INTEGER NOT NULL, record_key TEXT, record_label TEXT, PRIMARY KEY(content_id,ordinal)) WITHOUT ROWID;
CREATE UNIQUE INDEX content_records_key ON content_records(content_id,record_key) WHERE record_key IS NOT NULL;
CREATE INDEX content_records_label ON content_records(content_id,record_label) WHERE record_label IS NOT NULL;
CREATE TABLE content_dependencies(parent_id TEXT NOT NULL,child_id TEXT NOT NULL,
  PRIMARY KEY(parent_id,child_id), CHECK(parent_id<>child_id)) WITHOUT ROWID;
CREATE INDEX content_dependencies_child ON content_dependencies(child_id);
CREATE TRIGGER content_dependency_insert AFTER INSERT ON content_dependencies BEGIN
  UPDATE content SET unreferenced_at=NULL WHERE id=NEW.child_id;
END;
CREATE TRIGGER content_dependency_delete AFTER DELETE ON content_dependencies BEGIN
  UPDATE content SET unreferenced_at=CAST(strftime('%s','now') AS INTEGER)*1000 WHERE id=OLD.child_id
    AND NOT EXISTS(SELECT 1 FROM item_content WHERE content_id=OLD.child_id)
    AND NOT EXISTS(SELECT 1 FROM content_dependencies WHERE child_id=OLD.child_id);
END;
DROP TRIGGER IF EXISTS item_content_delete;
CREATE TRIGGER item_content_delete AFTER DELETE ON item_content BEGIN
  UPDATE content SET unreferenced_at=CAST(strftime('%s','now') AS INTEGER)*1000 WHERE id=OLD.content_id
    AND NOT EXISTS(SELECT 1 FROM item_content WHERE content_id=OLD.content_id)
    AND NOT EXISTS(SELECT 1 FROM content_dependencies WHERE child_id=OLD.content_id);
END;
DROP TRIGGER IF EXISTS content_delete;
CREATE TRIGGER content_delete AFTER DELETE ON content BEGIN
  DELETE FROM content_chunks WHERE content_id=OLD.id;
  DELETE FROM content_records WHERE content_id=OLD.id;
  DELETE FROM content_collections WHERE content_id=OLD.id;
  DELETE FROM content_dependencies WHERE parent_id=OLD.id;
END;
