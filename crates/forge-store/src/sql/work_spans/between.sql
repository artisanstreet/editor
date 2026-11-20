SELECT first, last, run_id, items, worked FROM work_spans WHERE thread_id=?1 AND last>?2 AND first<?3 ORDER BY first
