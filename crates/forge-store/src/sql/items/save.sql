INSERT INTO items (id, thread_id, position, seq, run_id, status, body,
    edit_added, edit_removed, kind_type, parent_id, work, outline)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, json_extract(?7, '$.kind.type'), ?10, ?11, ?12)
ON CONFLICT (id) DO UPDATE SET seq = excluded.seq, run_id = excluded.run_id,
    status = excluded.status, body = excluded.body,
    edit_added = excluded.edit_added, edit_removed = excluded.edit_removed,
    kind_type = excluded.kind_type, parent_id = excluded.parent_id,
    work = excluded.work, outline = excluded.outline
