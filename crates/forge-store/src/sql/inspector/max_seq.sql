SELECT MAX(COALESCE((SELECT MAX(seq) FROM items WHERE thread_id=?1),0),COALESCE((SELECT MAX(seq) FROM runs WHERE thread_id=?1),0))
