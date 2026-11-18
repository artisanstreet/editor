SELECT t.thread_id, t.item_id, t.created_at, i.position,
       json_extract(h.body, '$.title'), json_extract(h.body, '$.project_id'), t.text
FROM item_text t
JOIN items i ON i.rowid = t.rowid
JOIN threads h ON h.id = t.thread_id
WHERE t.kind IN ('user', 'assistant')
  AND i.parent_id IS NULL AND i.work = 0
  AND COALESCE(json_extract(h.body, '$.archived'), 0) = 0
  AND json_extract(h.body, '$.parent_thread_id') IS NULL
ORDER BY t.rowid DESC
LIMIT ?1
