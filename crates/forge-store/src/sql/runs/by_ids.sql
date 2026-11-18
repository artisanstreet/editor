SELECT body FROM runs WHERE id IN (SELECT value FROM json_each(?1))
