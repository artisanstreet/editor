SELECT json_extract(body, '$.kind.label') FROM items
WHERE thread_id = ?1 AND id != ?2 AND kind_type = 'agent'
  AND status IN ('running', 'awaiting_user')
  AND json_extract(body, '$.kind.label') IS NOT NULL
