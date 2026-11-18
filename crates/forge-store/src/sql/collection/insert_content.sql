INSERT INTO content(id,bytes,lines,diff_added,diff_removed,unreferenced_at) VALUES(?1,?2,?3,0,0,?4) ON CONFLICT(id) DO NOTHING
