ALTER TABLE items ADD COLUMN kind_type TEXT;
ALTER TABLE items ADD COLUMN parent_id TEXT;
UPDATE items SET kind_type=json_extract(body,'$.kind.type'),parent_id=json_extract(body,'$.parent_id');
DROP INDEX IF EXISTS items_pending_input;
DROP INDEX IF EXISTS items_pending_inputs;
DROP INDEX IF EXISTS items_thread_kind_position;
DROP INDEX IF EXISTS items_thread_parent_position;
DROP INDEX IF EXISTS items_agent_recipient_position;
CREATE INDEX items_pending_input ON items(thread_id,position) WHERE status='running' AND kind_type='user_message';
CREATE INDEX items_pending_inputs ON items(thread_id,seq) WHERE status='running' AND kind_type='user_message';
CREATE INDEX items_thread_kind_position ON items(thread_id,kind_type,position);
CREATE INDEX items_thread_parent_position ON items(thread_id,parent_id,position);
CREATE INDEX items_agent_recipient_position ON items(thread_id,json_extract(body,'$.kind.tool.recipient'),position)
 WHERE parent_id IS NULL AND json_extract(body,'$.kind.tool.kind')='agent_message';
