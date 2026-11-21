SELECT EXISTS(SELECT 1 FROM items WHERE thread_id=?1 AND status='awaiting_user')
