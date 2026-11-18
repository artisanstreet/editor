CREATE INDEX items_thread_parent_position ON items(thread_id,json_extract(body,'$.parent_id'),position);
CREATE INDEX items_agent_recipient_position ON items(thread_id,json_extract(body,'$.kind.tool.recipient'),position)
 WHERE json_extract(body,'$.parent_id') IS NULL AND json_extract(body,'$.kind.tool.kind')='agent_message';
