CREATE INDEX items_awaiting_position ON items(position) WHERE status = 'awaiting_user';
CREATE INDEX items_pending_input ON items(thread_id, position)
    WHERE status = 'running' AND json_extract(body, '$.kind.type') = 'user_message';
