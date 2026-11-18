INSERT INTO continues (thread_id, seq, reason) VALUES (?1, ?2, ?3)
ON CONFLICT (thread_id) DO NOTHING
