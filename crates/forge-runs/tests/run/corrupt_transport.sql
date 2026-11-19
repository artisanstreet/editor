INSERT INTO kv (key, seq, value) VALUES ('claude_transport', 1, 'not a setting')
ON CONFLICT (key) DO UPDATE SET value = excluded.value
