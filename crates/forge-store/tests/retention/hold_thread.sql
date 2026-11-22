CREATE TABLE held_threads(id TEXT PRIMARY KEY);
CREATE TRIGGER hold_thread BEFORE DELETE ON threads WHEN OLD.id IN (SELECT id FROM held_threads)
BEGIN SELECT RAISE(ABORT, 'the thread is held'); END;
