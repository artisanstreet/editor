INSERT INTO content_collections(content_id,count,authored_bytes,structural_bytes) VALUES(?1,?2,?3,?4) ON CONFLICT(content_id) DO NOTHING
