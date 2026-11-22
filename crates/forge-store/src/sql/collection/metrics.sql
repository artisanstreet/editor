SELECT r.count,c.bytes,c.lines,r.authored_bytes,r.structural_bytes FROM content_collections r JOIN content c ON c.id=r.content_id WHERE r.content_id=?1
