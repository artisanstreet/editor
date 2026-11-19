UPDATE items SET body = json_remove(body, '$.kind.label') WHERE id = ?1
