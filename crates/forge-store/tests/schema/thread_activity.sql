SELECT seq,json_extract(body,'$.activity_at'),json_extract(body,'$.updated_at') FROM threads WHERE id=?1
