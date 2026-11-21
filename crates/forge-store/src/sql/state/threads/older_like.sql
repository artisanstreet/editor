SELECT t.body FROM threads t JOIN projects p ON p.id=json_extract(t.body,'$.project_id')
WHERE COALESCE(json_extract(t.body,'$.archived'),0)=?1
 AND COALESCE(json_extract(p.body,'$.removed'),0)=0
 AND (json_extract(t.body,'$.activity_seq'),t.id)<(?2,?3)
AND (?4='' OR (COALESCE(json_extract(t.body,'$.title'),'') || ' ' || json_extract(p.body,'$.name')) LIKE '%' || ?4 || '%' ESCAPE '\')
ORDER BY json_extract(t.body,'$.activity_seq') DESC,t.id DESC LIMIT ?5
