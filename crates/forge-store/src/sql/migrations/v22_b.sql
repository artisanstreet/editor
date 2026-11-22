CREATE INDEX items_transcript_rows ON items(thread_id,position) WHERE parent_id IS NULL AND work=0;
