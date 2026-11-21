SELECT position, work, run_id FROM items WHERE thread_id=?1 AND parent_id IS NULL AND position<?2 ORDER BY position DESC LIMIT 1
