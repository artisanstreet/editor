INSERT INTO secrets (key, value) VALUES (?1, ?2) ON CONFLICT (key) DO NOTHING
