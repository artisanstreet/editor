SELECT EXISTS (
    SELECT 1 FROM threads
    WHERE seq > ?1
      AND json_extract(body, '$.parent_thread_id') IN (SELECT value FROM json_each(?2))
      AND id NOT IN (SELECT value FROM json_each(?2))
)
