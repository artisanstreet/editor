SELECT COUNT(*), COALESCE(SUM(kind_type IN ('tool','agent','plan')), 0), MIN(position), MAX(position)
FROM items INDEXED BY items_work_spans
WHERE thread_id=?1 AND parent_id IS NULL AND work=1 AND position>=?2 AND position<=?3
