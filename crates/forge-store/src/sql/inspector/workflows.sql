SELECT json_object('id',id,'status',status,'name',json_extract(body,'$.kind.tool.name')) FROM items
 WHERE thread_id=?1 AND status IN ('running','awaiting_user')
    AND json_extract(body,'$.kind.tool.kind')='workflow' ORDER BY position DESC
