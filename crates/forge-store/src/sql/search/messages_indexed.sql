SELECT t.thread_id, t.item_id, t.created_at, i.position,
       json_extract(h.body, '$.title'), json_extract(h.body, '$.project_id'), t.text, t.folded
FROM message_search s
JOIN item_text t ON t.rowid = s.rowid
JOIN items i ON i.rowid = t.rowid
JOIN threads h ON h.id = t.thread_id
WHERE message_search MATCH ?1
  AND t.kind IN ('user', 'assistant')
  AND i.parent_id IS NULL AND i.work = 0
  AND COALESCE(json_extract(h.body, '$.archived'), 0) = 0
  AND json_extract(h.body, '$.parent_thread_id') IS NULL
ORDER BY s.rowid DESC
LIMIT ?2
