WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 400000)
INSERT INTO authorized_keys (id, name, public_key, created_at)
SELECT 'device-' || i, 'Laptop', 'key-' || i, 0 FROM n;
