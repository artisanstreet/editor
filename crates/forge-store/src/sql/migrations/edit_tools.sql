SELECT id,body FROM items WHERE status='done' AND json_extract(body,'$.kind.tool.kind')='edit'
