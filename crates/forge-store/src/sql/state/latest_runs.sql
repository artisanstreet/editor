SELECT body FROM runs r WHERE r.thread_id IN (SELECT value FROM json_each(?1))
AND r.rowid=(SELECT rowid FROM runs WHERE thread_id=r.thread_id ORDER BY rowid DESC LIMIT 1)
