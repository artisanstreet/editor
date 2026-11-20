SELECT COUNT(*)
FROM threads t
WHERE t.id IN (SELECT value FROM json_each(?2))
  AND MAX(COALESCE(json_extract(t.body, '$.activity_at'), 0),
          COALESCE(json_extract(t.body, '$.updated_at'), 0),
          COALESCE(json_extract(t.body, '$.created_at'), 0)) <= ?1
  AND json_extract(t.body, '$.parked_until') IS NULL
  AND NOT EXISTS (SELECT 1 FROM runs r WHERE r.thread_id = t.id AND r.status = 'running')
  AND NOT EXISTS (SELECT 1 FROM items i WHERE i.thread_id = t.id AND i.status IN ('running', 'awaiting_user'))
  AND NOT EXISTS (SELECT 1 FROM continues c WHERE c.thread_id = t.id)
  AND NOT EXISTS (SELECT 1 FROM drafts d WHERE d.scope = 'thread:' || t.id AND d.body <> 'null')
