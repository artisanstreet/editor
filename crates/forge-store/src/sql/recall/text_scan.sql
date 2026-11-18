SELECT t.thread_id,json_extract(h.body,'$.title'),t.item_id,t.kind,t.created_at,
       substr(t.text,1,200)
FROM item_text t JOIN threads h ON h.id=t.thread_id
WHERE instr(lower(t.text),lower(?1))>0 AND ?4 IS NOT NULL__SCOPE__
ORDER BY t.rowid DESC LIMIT ?2 OFFSET ?3
