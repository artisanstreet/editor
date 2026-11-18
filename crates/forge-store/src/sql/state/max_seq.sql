SELECT coalesce(max(seq), 0) FROM (
SELECT max(seq) AS seq FROM projects UNION ALL
SELECT max(seq) FROM threads UNION ALL
SELECT max(seq) FROM items UNION ALL
SELECT max(seq) FROM runs UNION ALL
SELECT max(seq) FROM removed_threads UNION ALL
SELECT max(seq) FROM kv)
