COALESCE((SELECT json_extract(r.body,'$.engine') FROM items i JOIN runs r ON r.id=i.run_id
    WHERE i.rowid=t.rowid),json_extract(h.body,'$.config.engine'))=?
