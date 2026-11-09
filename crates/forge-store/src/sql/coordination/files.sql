WITH RECURSIVE lines(row_id,thread_id,item_id,created_at,remaining,line) AS (
    SELECT t.rowid,t.thread_id,t.item_id,t.created_at,t.paths||char(10),''
    FROM item_text t JOIN threads h ON h.id=t.thread_id
    WHERE t.paths<>'' AND (?1 IS NULL OR json_extract(h.body,'$.project_id')=?1)
    AND EXISTS(SELECT 1 FROM json_each(?2) needle
        WHERE instr(lower(replace(t.paths,char(92),'/')),lower(needle.value))>0)
    UNION ALL
    SELECT row_id,thread_id,item_id,created_at,
        substr(remaining,instr(remaining,char(10))+1),
        substr(remaining,1,instr(remaining,char(10))-1)
    FROM lines WHERE remaining<>''
), entries AS (
    SELECT row_id,thread_id,item_id,created_at,line,
        substr(line,1,instr(line,char(9))-1) AS operation,
        replace(substr(line,instr(line,char(9))+1),char(92),'/') AS path
    FROM lines WHERE line<>'' AND instr(line,char(9))>0
), normalized AS (
    SELECT row_id,thread_id,item_id,created_at,line,operation,path,
        CASE WHEN substr(path,2,1)=':' OR substr(path,1,2)='//' THEN lower(path) ELSE path END AS matched_path
    FROM entries
)
SELECT e.row_id,e.line,e.thread_id,json_extract(h.body,'$.title'),e.item_id,e.operation,e.path,e.created_at
FROM normalized e JOIN threads h ON h.id=e.thread_id
WHERE EXISTS(SELECT 1 FROM json_each(?2) needle
    WHERE (?3=1 AND instr(e.matched_path,needle.value)>0) OR (?3=0 AND e.matched_path=needle.value))
AND (?4 IS NULL OR (e.row_id,e.line)<(?4,?5))
ORDER BY e.row_id DESC,e.line DESC LIMIT ?6;
