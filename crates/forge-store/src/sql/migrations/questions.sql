SELECT id,body FROM items WHERE json_extract(body,'$.kind.type')='question'
