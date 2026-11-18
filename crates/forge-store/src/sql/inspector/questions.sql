SELECT json_object('id',id,'questions',json(json_extract(body,'$.kind.questions'))) FROM items
    WHERE thread_id=?1 AND status='awaiting_user' AND kind_type='question'
    AND json_extract(body,'$.kind.answers') IS NULL ORDER BY position ASC
