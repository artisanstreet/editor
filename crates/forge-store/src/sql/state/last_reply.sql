SELECT body FROM items WHERE thread_id=?1 AND kind_type='assistant_message'
    AND parent_id IS NULL ORDER BY position DESC LIMIT 1
