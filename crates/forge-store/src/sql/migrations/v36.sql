CREATE INDEX items_cancelled_by_run ON items(run_id,position) WHERE status='cancelled';
