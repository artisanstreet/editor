SELECT seq,record_id,owner,project_id,kind,revision,created_at FROM coordination_events
WHERE seq>?1 AND (?2 IS NULL OR project_id=?2)
ORDER BY seq LIMIT ?3;
