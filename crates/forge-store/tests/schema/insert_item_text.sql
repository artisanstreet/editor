INSERT INTO item_text(rowid,thread_id,item_id,kind,created_at,text,output,paths)
SELECT rowid,thread_id,id,'assistant',1,?2,'','' FROM items WHERE id=?1
