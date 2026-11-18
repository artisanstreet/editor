SELECT start,data FROM content_chunks WHERE content_id=?1 AND start>=?2 AND start<?3 ORDER BY start
