SELECT body FROM items WHERE thread_id=?1 AND parent_id IS NULL
    AND kind_type='user_message' AND status='cancelled'
    AND position > (SELECT coalesce(max(position), 0) FROM items
        WHERE thread_id=?1 AND parent_id IS NULL
        AND kind_type='user_message' AND status!='cancelled')
    ORDER BY position
