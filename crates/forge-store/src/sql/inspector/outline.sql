SELECT id, position, outline FROM items INDEXED BY items_outline
WHERE thread_id = ?1 AND outline IS NOT NULL
ORDER BY position DESC
LIMIT ?2
