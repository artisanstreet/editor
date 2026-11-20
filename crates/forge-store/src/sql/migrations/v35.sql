CREATE TABLE resource_capture (
    id            INTEGER PRIMARY KEY CHECK (id = 1),
    from_position INTEGER NOT NULL
);
INSERT INTO resource_capture (id, from_position)
SELECT 1, COALESCE(MAX(position), 0) FROM items;
