SELECT id FROM items WHERE thread_id=?1 AND kind_type='agent'
AND json_extract(body,'$.kind.agent_id')=?2 ORDER BY position DESC LIMIT 1
