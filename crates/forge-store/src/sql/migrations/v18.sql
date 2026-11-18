DELETE FROM items WHERE kind_type='compaction'
AND json_extract(body,'$.kind.tokens_before') IS NULL
AND EXISTS(SELECT 1 FROM items c WHERE c.run_id=items.run_id AND c.kind_type='compaction'
  AND (json_extract(c.body,'$.kind.tokens_before') IS NOT NULL OR c.position<items.position));
