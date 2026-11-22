ALTER TABLE drafts ADD COLUMN revision INTEGER NOT NULL DEFAULT 1;
CREATE INDEX threads_activity ON threads(COALESCE(json_extract(body,'$.archived'),0),json_extract(body,'$.activity_seq') DESC,id DESC);
CREATE INDEX items_thread_seq ON items(thread_id,seq);
CREATE INDEX items_thread_kind_position ON items(thread_id,json_extract(body,'$.kind.type'),position);
