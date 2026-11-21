SELECT body FROM items WHERE kind_type='agent' AND status IN ('running','awaiting_user')
    AND json_extract(body,'$.kind.thread') IS NOT NULL
