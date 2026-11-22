CREATE INDEX items_work_spans ON items(thread_id,position,run_id,kind_type,parent_id,work) WHERE parent_id IS NULL AND work=1;
