UPDATE authorized_keys SET last_seen = ?2 WHERE id = ?1
AND (last_seen IS NULL OR last_seen < ?2 - 60000)
