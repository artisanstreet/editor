INSERT INTO content_diffs(content_id,added,removed) VALUES(?1,?2,?3) ON CONFLICT(content_id) DO NOTHING
