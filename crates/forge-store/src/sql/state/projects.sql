SELECT body FROM projects WHERE COALESCE(json_extract(body,'$.removed'),0)=0 ORDER BY rowid
