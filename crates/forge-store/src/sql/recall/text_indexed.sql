SELECT t.thread_id,json_extract(h.body,'$.title'),t.item_id,t.kind,t.created_at,
       snippet(item_search,0,'**','**','…',24)
FROM item_search s JOIN item_text t ON t.rowid=s.rowid JOIN threads h ON h.id=t.thread_id
WHERE item_search MATCH ?1__SCOPE__
ORDER BY instr(lower(t.text),lower(?4))>0 DESC,
       bm25(item_search,1.0,0.5) * CASE WHEN t.kind IN ('user','assistant','agent','plan','question')
           THEN 3.0 ELSE 1.0 END
LIMIT ?2 OFFSET ?3
