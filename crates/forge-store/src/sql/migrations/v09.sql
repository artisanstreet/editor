ALTER TABLE items ADD COLUMN edit_added INTEGER;
ALTER TABLE items ADD COLUMN edit_removed INTEGER;
CREATE TABLE thread_edit_totals(thread_id TEXT PRIMARY KEY, added INTEGER NOT NULL, removed INTEGER NOT NULL, operations INTEGER NOT NULL);
CREATE TRIGGER item_edit_insert AFTER INSERT ON items WHEN NEW.edit_added IS NOT NULL BEGIN
  INSERT INTO thread_edit_totals VALUES(NEW.thread_id,NEW.edit_added,NEW.edit_removed,1)
  ON CONFLICT(thread_id) DO UPDATE SET added=added+excluded.added,removed=removed+excluded.removed,operations=operations+1;
END;
CREATE TRIGGER item_edit_update AFTER UPDATE OF edit_added,edit_removed ON items
 WHEN OLD.edit_added IS NOT NEW.edit_added OR OLD.edit_removed IS NOT NEW.edit_removed BEGIN
  INSERT INTO thread_edit_totals VALUES(NEW.thread_id,COALESCE(NEW.edit_added,0)-COALESCE(OLD.edit_added,0),
     COALESCE(NEW.edit_removed,0)-COALESCE(OLD.edit_removed,0),(NEW.edit_added IS NOT NULL)-(OLD.edit_added IS NOT NULL))
  ON CONFLICT(thread_id) DO UPDATE SET added=added+excluded.added,removed=removed+excluded.removed,operations=operations+excluded.operations;
END;
CREATE TRIGGER item_edit_delete AFTER DELETE ON items WHEN OLD.edit_added IS NOT NULL BEGIN
  UPDATE thread_edit_totals SET added=added-OLD.edit_added,removed=removed-OLD.edit_removed,operations=operations-1 WHERE thread_id=OLD.thread_id;
END;
