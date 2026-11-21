UPDATE runs SET body=json_set(body,'$.input_cutoff',seq);
CREATE INDEX items_pending_inputs ON items(thread_id,seq)
 WHERE status='running' AND json_extract(body,'$.kind.type')='user_message';
