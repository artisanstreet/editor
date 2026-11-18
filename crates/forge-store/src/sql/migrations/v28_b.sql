CREATE INDEX items_outline ON items(thread_id,position,id,outline) WHERE outline IS NOT NULL;
