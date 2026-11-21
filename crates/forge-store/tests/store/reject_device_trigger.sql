CREATE TRIGGER reject_device BEFORE INSERT ON authorized_keys BEGIN SELECT RAISE(ABORT, 'injected device failure'); END;
