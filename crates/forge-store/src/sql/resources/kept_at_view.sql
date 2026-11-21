SELECT items.position <= resource_capture.from_position
FROM items, resource_capture
WHERE items.id = ?1 AND items.thread_id = ?2
