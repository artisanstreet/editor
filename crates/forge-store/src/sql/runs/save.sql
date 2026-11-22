INSERT INTO runs (id, thread_id, seq, status, body) VALUES (?1, ?2, ?3, ?4, ?5)
ON CONFLICT (id) DO UPDATE SET seq = excluded.seq, status = excluded.status,
    body = excluded.body
