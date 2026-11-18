SELECT t.thread_id,json_extract(h.body,'$.title'),t.item_id,t.paths,t.created_at
FROM item_text t JOIN threads h ON h.id=t.thread_id
WHERE t.paths<>'' AND __PATHS____SCOPE__
ORDER BY t.rowid DESC LIMIT ?2
