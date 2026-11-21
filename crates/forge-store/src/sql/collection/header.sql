SELECT c.bytes,c.lines,r.count FROM content c JOIN content_collections r ON r.content_id=c.id WHERE c.id=?1
