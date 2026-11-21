SELECT EXISTS(SELECT 1 FROM items
 WHERE thread_id=?1 AND kind_type='user_message' AND position>?2
    AND json_extract(body,'$.parent_id') IS NULL
    AND NOT (run_id IS NOT NULL AND json_extract(body,'$.kind.steer')=1)
    AND json_extract(body,'$.kind.from_agent') IS NULL)
