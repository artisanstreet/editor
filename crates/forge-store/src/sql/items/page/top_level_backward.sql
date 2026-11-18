SELECT body FROM items WHERE thread_id=?1 AND parent_id IS NULL AND position>?2 AND position<?3 ORDER BY position DESC LIMIT ?4
