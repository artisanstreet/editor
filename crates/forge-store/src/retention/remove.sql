DELETE FROM coordination_events WHERE record_id IN (
    SELECT id FROM coordination_records
    WHERE owner = ?1
       OR (kind = 'message' AND json_extract(body, '$.content.recipient') = ?1)
       OR (kind = 'watch' AND json_extract(body, '$.content.target') = ?1));
DELETE FROM coordination_records
WHERE owner = ?1
   OR (kind = 'message' AND json_extract(body, '$.content.recipient') = ?1)
   OR (kind = 'watch' AND json_extract(body, '$.content.target') = ?1);
DELETE FROM items WHERE thread_id = ?1;
DELETE FROM work_spans WHERE thread_id = ?1;
DELETE FROM outline_revisions WHERE thread_id = ?1;
DELETE FROM runs WHERE thread_id = ?1;
DELETE FROM continues WHERE thread_id = ?1;
DELETE FROM thread_edit_totals WHERE thread_id = ?1;
DELETE FROM drafts WHERE scope = 'thread:' || ?1;
DELETE FROM threads WHERE id = ?1;
