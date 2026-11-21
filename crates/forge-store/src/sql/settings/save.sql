INSERT INTO kv (key, seq, value) VALUES (?1, ?2, ?3)
ON CONFLICT (key) DO UPDATE SET seq = excluded.seq, value = excluded.value
