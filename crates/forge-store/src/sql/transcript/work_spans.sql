SELECT position,run_id,kind_type FROM items INDEXED BY items_work_spans
WHERE thread_id=?1 AND parent_id IS NULL AND work=1 AND position>?2 AND position<?3
ORDER BY position
