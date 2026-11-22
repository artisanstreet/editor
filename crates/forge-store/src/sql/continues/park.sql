INSERT INTO continues (thread_id, seq, reason, due_at) VALUES (?1, ?2, ?3, ?4)
ON CONFLICT (thread_id) DO UPDATE SET seq = excluded.seq,
  reason = excluded.reason, due_at = excluded.due_at
