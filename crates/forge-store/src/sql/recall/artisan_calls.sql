SELECT body FROM items WHERE kind_type='tool'
AND json_extract(body,'$.kind.tool.kind')='mcp'
AND json_extract(body,'$.kind.tool.server')='artisan'
