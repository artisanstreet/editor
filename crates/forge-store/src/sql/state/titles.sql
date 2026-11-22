SELECT id, json_extract(body,'$.title') FROM threads
    WHERE json_extract(body,'$.title') IS NOT NULL
    ORDER BY json_extract(body,'$.activity_seq') DESC, seq DESC
