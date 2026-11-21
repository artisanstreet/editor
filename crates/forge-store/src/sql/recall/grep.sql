SELECT t.rowid,t.thread_id,json_extract(h.body,'$.title'),t.item_id,t.kind,t.text,__OUTPUT__,t.created_at
FROM item_text t JOIN threads h ON h.id=t.thread_id
WHERE t.rowid<=?1__SCOPE__ ORDER BY t.rowid DESC LIMIT __LIMIT__
