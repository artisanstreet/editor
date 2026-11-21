SELECT start FROM content_chunks WHERE content_id=?1 AND start<=?2 ORDER BY start DESC LIMIT 1
