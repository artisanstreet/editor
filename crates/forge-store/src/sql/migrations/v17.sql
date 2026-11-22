UPDATE items SET body=json_remove(body,'$.kind.tool.recipient_label')
  WHERE kind_type='tool' AND json_extract(body,'$.kind.tool.kind')='agent_message';
UPDATE items SET body=json_set(body,'$.kind.tool.target',(SELECT a.id FROM items a
    WHERE a.thread_id=items.thread_id AND a.kind_type='agent'
      AND json_extract(a.body,'$.kind.agent_id')=json_extract(items.body,'$.kind.tool.recipient')
    ORDER BY a.position DESC LIMIT 1))
  WHERE kind_type='tool' AND json_extract(body,'$.kind.tool.kind')='agent_message'
    AND EXISTS(SELECT 1 FROM items a WHERE a.thread_id=items.thread_id AND a.kind_type='agent'
      AND json_extract(a.body,'$.kind.agent_id')=json_extract(items.body,'$.kind.tool.recipient'));
DROP INDEX IF EXISTS items_agent_recipient_position;
CREATE INDEX items_agent_message_target ON items(thread_id,json_extract(body,'$.kind.tool.target'),position)
 WHERE parent_id IS NULL AND json_extract(body,'$.kind.tool.kind')='agent_message';
