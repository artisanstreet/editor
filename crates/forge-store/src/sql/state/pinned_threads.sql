SELECT body FROM threads WHERE id IN (
    SELECT thread_id FROM runs WHERE status='running'
    UNION SELECT thread_id FROM items WHERE status='awaiting_user')
  AND id NOT IN (SELECT value FROM json_each(?1))
ORDER BY json_extract(body,'$.activity_seq') DESC LIMIT ?2
