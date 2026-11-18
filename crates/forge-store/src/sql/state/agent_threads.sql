SELECT body FROM threads WHERE json_extract(body,'$.parent_thread_id')=?1
    AND json_extract(body,'$.spawned_by') IS NOT NULL ORDER BY seq
