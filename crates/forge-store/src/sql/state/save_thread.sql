INSERT INTO threads (id, seq, body) VALUES (?1, ?2, ?3)
ON CONFLICT (id) DO UPDATE SET seq = excluded.seq, body = excluded.body
