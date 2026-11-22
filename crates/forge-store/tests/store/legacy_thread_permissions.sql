UPDATE threads SET body = json_set(json_remove(body, '$.config.permissions'), '$.config.access', 'workspace', '$.config.approvals', ?2) WHERE id = ?1;
