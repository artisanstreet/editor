INSERT INTO items(id,thread_id,position,seq,status,body,kind_type) VALUES(?1,'t',?2,?2,?3,?4,json_extract(?4,'$.kind.type'))
