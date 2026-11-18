SELECT json_extract(body,'$.kind.kind.state'),json_quote(json_extract(body,'$.kind.text')) FROM items
    WHERE thread_id=?1 AND parent_id IS NULL
    AND kind_type='notice' AND json_extract(body,'$.kind.kind.type')='goal'
    ORDER BY seq DESC LIMIT 1
