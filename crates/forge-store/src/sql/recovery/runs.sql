SELECT body FROM runs WHERE status = 'running'
AND (?1 IS NULL OR thread_id IN (SELECT value FROM json_each(?1)));
