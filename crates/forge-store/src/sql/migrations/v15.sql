CREATE TABLE source_indexes(source_id TEXT NOT NULL, format TEXT NOT NULL CHECK(format IN ('literal','markdown')), version INTEGER NOT NULL,
 metadata_id TEXT NOT NULL, leaf_count INTEGER NOT NULL, PRIMARY KEY(source_id,format,version)) WITHOUT ROWID;
CREATE TABLE source_index_leaves(source_id TEXT NOT NULL, format TEXT NOT NULL CHECK(format IN ('literal','markdown')), version INTEGER NOT NULL, ordinal INTEGER NOT NULL,
 source_start INTEGER NOT NULL, source_end INTEGER NOT NULL, metadata_start INTEGER NOT NULL, metadata_end INTEGER NOT NULL,
 PRIMARY KEY(source_id,format,version,ordinal)) WITHOUT ROWID;
CREATE TRIGGER source_index_delete AFTER DELETE ON content BEGIN
 DELETE FROM source_index_leaves WHERE source_id=OLD.id;
 DELETE FROM source_indexes WHERE source_id=OLD.id;
END;
