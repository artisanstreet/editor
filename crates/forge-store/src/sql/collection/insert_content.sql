INSERT INTO content(id,bytes,lines,unreferenced_at) VALUES(?1,?2,?3,?4) ON CONFLICT(id) DO NOTHING
