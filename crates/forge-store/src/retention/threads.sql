SELECT t.id,
       json_extract(t.body, '$.parent_thread_id'),
       MAX(COALESCE(json_extract(t.body, '$.activity_at'), 0),
           COALESCE(json_extract(t.body, '$.updated_at'), 0),
           COALESCE(json_extract(t.body, '$.created_at'), 0)) <= ?1
       AND json_extract(t.body, '$.parked_until') IS NULL
       AND t.id NOT IN (SELECT thread_id FROM runs WHERE status = 'running')
       AND t.id NOT IN (SELECT thread_id FROM items WHERE status IN ('running', 'awaiting_user'))
       AND t.id NOT IN (SELECT thread_id FROM continues)
FROM threads t
