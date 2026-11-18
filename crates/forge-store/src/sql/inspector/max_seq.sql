SELECT COALESCE(MAX(seq),0) FROM (SELECT seq FROM items WHERE thread_id=?1 UNION ALL SELECT seq FROM runs WHERE thread_id=?1)
