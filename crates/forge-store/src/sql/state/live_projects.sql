SELECT count(*) FROM projects WHERE COALESCE(json_extract(body,'$.removed'),0)=0
