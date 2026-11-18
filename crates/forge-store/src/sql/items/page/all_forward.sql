SELECT body FROM items WHERE thread_id=?1 AND position>?2 AND position<?3 ORDER BY position ASC LIMIT ?4
