SELECT seq, kind, CASE WHEN length(CAST(body AS BLOB)) <= 16777216 THEN body END, key FROM (
SELECT seq, 'project' AS kind, body, NULL AS key FROM projects WHERE seq > ?1
UNION ALL SELECT seq, 'thread', body, NULL FROM threads WHERE seq > ?1
UNION ALL SELECT seq, 'thread_removed', json_quote(id), NULL FROM removed_threads WHERE seq > ?1
UNION ALL SELECT seq,
    CASE WHEN ?3 AND (?4 IS NULL OR thread_id != ?4) THEN 'item_state' ELSE 'item' END,
    CASE WHEN ?3 AND (?4 IS NULL OR thread_id != ?4)
        THEN json_object('id',id,'thread_id',thread_id,'status',status) ELSE body END,
    NULL FROM items WHERE seq > ?1
UNION ALL SELECT seq, 'run', body, NULL FROM runs WHERE seq > ?1
UNION ALL SELECT seq, 'setting', value, key FROM kv WHERE seq > ?1)
ORDER BY 1 LIMIT ?2
