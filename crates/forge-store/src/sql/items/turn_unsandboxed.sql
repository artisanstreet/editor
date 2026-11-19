SELECT EXISTS(
    SELECT 1 FROM items JOIN runs ON runs.id=items.run_id
    WHERE items.thread_id=?1 AND items.kind_type='notice'
    AND json_extract(items.body,'$.kind.kind.type')='unsandboxed'
    AND runs.rowid>=(
        SELECT max(rowid) FROM runs
        WHERE thread_id=?1 AND json_extract(body,'$.continuation') IS NULL
    )
)
