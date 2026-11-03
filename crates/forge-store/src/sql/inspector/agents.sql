SELECT json_object('id',id,'label',COALESCE(json_extract(body,'$.kind.label'),''),
            'description',json_extract(body,'$.kind.description'),'status',status,
            'engine',COALESCE(json_extract(body,'$.kind.engine'),
                (SELECT json_extract(t.body,'$.config.engine') FROM threads t WHERE t.id=?1)),
            'model',json_extract(body,'$.kind.model'),'thread',json_extract(body,'$.kind.thread'),
            'config',json(COALESCE(
                (SELECT json_extract(t.body,'$.config') FROM threads t WHERE t.id=json_extract(items.body,'$.kind.thread')),
                (SELECT json_extract(t.body,'$.config') FROM threads t WHERE t.id=?1))),
            'background',json(CASE WHEN COALESCE(json_extract(body,'$.kind.background'),0)
                OR (SELECT r.status FROM runs r WHERE r.id=items.run_id)!='running'
                THEN 'true' ELSE 'false' END)) FROM items
         WHERE thread_id=?1 AND kind_type='agent'
            AND status IN ('running','awaiting_user')
            AND COALESCE(json_extract(body,'$.kind.agent_id'),'') NOT IN (
                SELECT json_extract(body,'$.session_id') FROM runs
                WHERE thread_id=?1 AND json_extract(body,'$.session_id') IS NOT NULL
            ) ORDER BY position DESC
