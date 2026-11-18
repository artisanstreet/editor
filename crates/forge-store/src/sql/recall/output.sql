SELECT t.thread_id,json_extract(h.body,'$.title'),t.item_id,t.kind,t.created_at,
       substr(t.output,max(1,instr(lower(t.output),lower(?4))-80),240)
FROM item_text t JOIN threads h ON h.id=t.thread_id
WHERE instr(lower(t.output),lower(?4))>0 AND NOT (__TEXT__)__SCOPE__
ORDER BY t.rowid DESC LIMIT ?2 OFFSET ?3
