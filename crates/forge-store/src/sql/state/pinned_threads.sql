SELECT body FROM threads WHERE (id IN (
    SELECT thread_id FROM runs WHERE status='running'
    UNION SELECT thread_id FROM items WHERE status='awaiting_user')
  OR (COALESCE(json_extract(body,'$.archived'),0)=0
    AND json_extract(body,'$.parent_thread_id') IS NULL
    AND (COALESCE(json_extract(body,'$.marked_unread'),0)=1
      OR json_extract(body,'$.activity_seq')>COALESCE(json_extract(body,'$.read_seq'),0))
    AND COALESCE(json_extract(body,'$.settled_seq'),0)<json_extract(body,'$.activity_seq')))
  AND id NOT IN (SELECT value FROM json_each(?1))
ORDER BY json_extract(body,'$.activity_seq') DESC LIMIT ?2
