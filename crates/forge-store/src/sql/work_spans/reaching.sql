SELECT first, last, run_id, items, worked FROM work_spans WHERE thread_id=?1 AND first<=?2 ORDER BY first DESC LIMIT 1
