SELECT t.body FROM threads t JOIN projects p ON p.id=json_extract(t.body,'$.project_id')
WHERE COALESCE(json_extract(t.body,'$.archived'),0)=?1
 AND COALESCE(json_extract(p.body,'$.removed'),0)=0
 AND (json_extract(t.body,'$.activity_seq'),t.id)>(?2,?3)
AND t.rowid IN (SELECT rowid FROM threads_search WHERE threads_search MATCH ?4)
ORDER BY json_extract(t.body,'$.activity_seq') ASC,t.id ASC LIMIT ?5
