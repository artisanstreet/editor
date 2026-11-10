SELECT body FROM items WHERE status IN ('running','awaiting_user')
AND kind_type != 'user_message'
AND (?1 IS NULL OR thread_id IN (SELECT value FROM json_each(?1)))
ORDER BY position;
