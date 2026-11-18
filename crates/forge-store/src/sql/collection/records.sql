SELECT ordinal,start,end FROM content_records WHERE content_id=?1 AND ordinal>=?2 ORDER BY ordinal LIMIT ?3
