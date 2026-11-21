UPDATE threads SET body=json_set(body,'$.activity_at',COALESCE(
(SELECT json_extract(i.body,'$.updated_at') FROM items i
 WHERE i.thread_id=threads.id AND i.seq=json_extract(threads.body,'$.activity_seq')),
(SELECT json_extract(i.body,'$.created_at') FROM items i
 WHERE i.thread_id=threads.id AND i.position=json_extract(threads.body,'$.activity_seq')
   AND json_extract(i.body,'$.kind.type')='user_message'),
(SELECT COALESCE(json_extract(r.body,'$.ended_at'),json_extract(r.body,'$.started_at')) FROM runs r
 WHERE r.thread_id=threads.id AND r.seq<=json_extract(threads.body,'$.activity_seq')
 ORDER BY r.seq DESC LIMIT 1),
json_extract(body,'$.created_at'),0));
