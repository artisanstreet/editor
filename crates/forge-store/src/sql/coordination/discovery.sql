SELECT t.body,p.body,
    (SELECT r.body FROM runs r WHERE r.thread_id=t.id ORDER BY r.seq DESC LIMIT 1),
    EXISTS(SELECT 1 FROM items i WHERE i.thread_id=t.id AND i.status='awaiting_user'),w.body
FROM threads t
JOIN projects p ON p.id=json_extract(t.body,'$.project_id')
LEFT JOIN coordination_records w ON w.id='work:'||t.id
WHERE COALESCE(json_extract(p.body,'$.removed'),0)=0
AND (?1 IS NULL OR json_extract(t.body,'$.project_id')=?1)
AND (?2 IS NULL OR json_extract(t.body,'$.parent_thread_id')=?2)
AND (?3=1 OR json_extract(t.body,'$.spawned_by') IS NULL)
AND (?4=0 OR t.id IN (SELECT value FROM json_each(?5))
    OR EXISTS(SELECT 1 FROM items i WHERE i.thread_id=t.id AND i.status='awaiting_user')
    OR json_extract(w.body,'$.content.state') IN ('blocked','waiting'))
AND (?6 IS NULL OR COALESCE(json_extract(t.body,'$.archived'),0)=?6)
AND (?7='' OR instr(lower(COALESCE(json_extract(t.body,'$.title'),'')),lower(?7))>0
    OR instr(lower(json_extract(p.body,'$.name')),lower(?7))>0)
AND (?8 IS NULL OR t.id=?8 OR json_extract(t.body,'$.spawned_by')=?8)
AND (?9 IS NULL OR (json_extract(t.body,'$.activity_seq'),t.id)<(?9,?10))
AND (?11 IS NULL OR COALESCE(json_extract(w.body,'$.content.worktree'),json_extract(p.body,'$.path'))=?11)
AND (?12 IS NULL OR (COALESCE(json_extract(t.body,'$.marked_unread'),0)=1
    OR json_extract(t.body,'$.activity_seq')>json_extract(t.body,'$.read_seq'))=?12)
ORDER BY json_extract(t.body,'$.activity_seq') DESC,t.id DESC LIMIT ?13;
