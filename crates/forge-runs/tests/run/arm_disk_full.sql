BEGIN IMMEDIATE;
CREATE TRIGGER disk_full_insert BEFORE INSERT ON __TABLE__
    BEGIN SELECT RAISE(ABORT, 'database or disk is full'); END;
CREATE TRIGGER disk_full_update BEFORE UPDATE ON __TABLE__
    BEGIN SELECT RAISE(ABORT, 'database or disk is full'); END;
