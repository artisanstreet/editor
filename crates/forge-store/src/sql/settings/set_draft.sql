INSERT INTO drafts (scope, body, revision) VALUES (?1, ?2, ?3)
ON CONFLICT (scope) DO UPDATE SET body=excluded.body, revision=excluded.revision
