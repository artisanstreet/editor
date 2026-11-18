CREATE TRIGGER disk_full BEFORE INSERT ON items
    WHEN NEW.thread_id = '__THREAD__' AND NEW.kind_type = 'user_message'
    BEGIN SELECT RAISE(ABORT, 'database or disk is full'); END
