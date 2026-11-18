INSERT INTO authorized_keys (id, name, public_key, created_at)
VALUES (?1, ?2, ?3, ?4)
ON CONFLICT (public_key) DO UPDATE SET name = excluded.name
RETURNING id
