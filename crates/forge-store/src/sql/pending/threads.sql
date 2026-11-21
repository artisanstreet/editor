SELECT thread_id FROM (
    SELECT thread_id, seq FROM runnable_inputs
    UNION ALL
    SELECT thread_id, seq FROM continues WHERE due_at IS NULL OR due_at <= ?3
)
WHERE thread_id NOT IN (SELECT value FROM json_each(?1))
GROUP BY thread_id ORDER BY min(seq) LIMIT ?2
