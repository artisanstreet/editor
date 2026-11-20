SELECT body FROM items INDEXED BY items_transcript_rows WHERE thread_id=?1 AND parent_id IS NULL AND work=0
AND position>?2 AND position<?3 ORDER BY position ASC
