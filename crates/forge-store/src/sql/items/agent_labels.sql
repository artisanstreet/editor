SELECT json_extract(body, '$.kind.label') FROM items
WHERE thread_id = ?1 AND kind_type = 'agent'
  AND json_extract(body, '$.kind.label') IS NOT NULL
