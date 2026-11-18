WITH runnable_inputs AS (
    SELECT pending.thread_id, pending.seq FROM items pending
    WHERE pending.status = 'running' AND pending.kind_type = 'user_message'
      AND NOT EXISTS (
        SELECT 1 FROM runs latest WHERE latest.rowid = (
          SELECT rowid FROM runs WHERE thread_id = pending.thread_id ORDER BY rowid DESC LIMIT 1
        ) AND latest.status IN ('failed', 'refused')
          AND pending.seq <= json_extract(latest.body, '$.input_cutoff')
      )
)
