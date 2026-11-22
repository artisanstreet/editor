SELECT body FROM items WHERE thread_id=?1 AND native_id=?2 AND (?3 IS NULL OR run_id=?3) ORDER BY position DESC LIMIT 1
