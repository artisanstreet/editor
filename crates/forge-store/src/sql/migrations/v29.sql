CREATE TABLE IF NOT EXISTS authorized_keys_revision(revision INTEGER NOT NULL);
INSERT INTO authorized_keys_revision(revision)
SELECT 0 WHERE NOT EXISTS (SELECT 1 FROM authorized_keys_revision);
CREATE TRIGGER IF NOT EXISTS authorized_keys_added AFTER INSERT ON authorized_keys BEGIN
  UPDATE authorized_keys_revision SET revision = revision + 1;
END;
CREATE TRIGGER IF NOT EXISTS authorized_keys_removed AFTER DELETE ON authorized_keys BEGIN
  UPDATE authorized_keys_revision SET revision = revision + 1;
END;
CREATE TRIGGER IF NOT EXISTS authorized_keys_rekeyed AFTER UPDATE OF id, public_key ON authorized_keys BEGIN
  UPDATE authorized_keys_revision SET revision = revision + 1;
END;
