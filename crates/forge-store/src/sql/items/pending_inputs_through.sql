SELECT body FROM items WHERE thread_id=?1 AND position<=?2 AND status='running'
AND kind_type='user_message'
