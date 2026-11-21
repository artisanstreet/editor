SELECT image.value FROM items i, json_each(i.body, '$.kind.images') image
    WHERE i.seq > ?1 AND i.kind_type = 'user_message'
UNION SELECT json_extract(body, '$.kind.blob') FROM items WHERE seq > ?1 AND kind_type = 'image'
UNION SELECT image.value FROM drafts d, json_each(d.body, '$.images') image
