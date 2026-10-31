INSERT INTO removed_threads (id, seq) VALUES (?1, ?2)
ON CONFLICT (id) DO UPDATE SET seq = excluded.seq
