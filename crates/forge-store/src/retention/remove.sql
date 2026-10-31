DELETE FROM items WHERE thread_id = ?1;
DELETE FROM runs WHERE thread_id = ?1;
DELETE FROM continues WHERE thread_id = ?1;
DELETE FROM thread_edit_totals WHERE thread_id = ?1;
DELETE FROM drafts WHERE scope = 'thread:' || ?1;
DELETE FROM threads WHERE id = ?1;
