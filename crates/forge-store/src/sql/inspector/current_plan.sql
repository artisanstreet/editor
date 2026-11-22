SELECT position,json_extract(body,'$.kind.steps') FROM items
WHERE thread_id=?1 AND kind_type='plan'
   AND parent_id IS NULL ORDER BY position DESC LIMIT 1
