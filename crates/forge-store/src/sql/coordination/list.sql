SELECT body FROM coordination_records
WHERE (?1 IS NULL OR project_id=?1) AND (?2 IS NULL OR kind=?2)
AND (?3 IS NULL OR owner=?3)
AND (?4 IS NULL OR json_extract(body,'$.content.recipient')=?4)
AND (?5 IS NULL OR (updated_at,id)<(?5,?6))
ORDER BY updated_at DESC,id DESC LIMIT ?7;
