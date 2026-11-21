CREATE TRIGGER unreadable_park_insert AFTER INSERT ON continues
WHEN typeof(NEW.due_at) = 'integer'
BEGIN UPDATE continues SET due_at = NEW.due_at + 0.5 WHERE thread_id = NEW.thread_id; END;
CREATE TRIGGER unreadable_park_update AFTER UPDATE ON continues
WHEN typeof(NEW.due_at) = 'integer'
BEGIN UPDATE continues SET due_at = NEW.due_at + 0.5 WHERE thread_id = NEW.thread_id; END;
