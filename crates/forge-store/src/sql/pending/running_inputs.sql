SELECT body FROM items WHERE thread_id=?1 AND status='running' AND kind_type='user_message' ORDER BY position LIMIT 32
