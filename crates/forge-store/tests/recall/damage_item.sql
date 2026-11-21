UPDATE items SET body = json_set(body, '$.kind.text', json(?2)) WHERE id = ?1
