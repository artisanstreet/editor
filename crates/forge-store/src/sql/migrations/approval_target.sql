SELECT id,body FROM items WHERE id=?1 AND thread_id=?2
AND kind_type IN ('tool','plan') AND json_extract(body,'$.kind.approval') IS NULL
