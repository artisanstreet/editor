SELECT m.name FROM sqlite_schema m, pragma_table_info(m.name) c
WHERE m.type = 'table' AND c.name = 'body' ORDER BY m.name
