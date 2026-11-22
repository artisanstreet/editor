SELECT body FROM (
    SELECT body,position FROM items WHERE thread_id=?1 AND parent_id=?5 AND position>?2 AND position<?3
    UNION ALL
    SELECT body,position FROM items WHERE thread_id=?1 AND parent_id IS NULL
        AND json_extract(body,'$.kind.tool.kind')='agent_message' AND json_extract(body,'$.kind.tool.target')=?5 AND position>?2 AND position<?3
) ORDER BY position DESC LIMIT ?4
