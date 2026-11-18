SELECT json_object('id',id,'command',json_extract(body,'$.kind.tool.command'),
    'cwd',json_extract(body,'$.kind.tool.cwd'),'status',status,
    'background',json(CASE WHEN COALESCE(json_extract(body,'$.kind.tool.background'),0)
        OR (SELECT r.status FROM runs r WHERE r.id=items.run_id)!='running'
        THEN 'true' ELSE 'false' END),
    'watch',json(CASE WHEN COALESCE(json_extract(body,'$.kind.tool.watch'),0)
        THEN 'true' ELSE 'false' END)) FROM items
 WHERE thread_id=?1 AND status IN ('running','awaiting_user') AND json_extract(body,'$.kind.tool.kind')='shell' ORDER BY position DESC
