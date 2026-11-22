SELECT ordinal,source_start,source_end,metadata_start,metadata_end FROM source_index_leaves WHERE source_id=?1 AND version=?2 AND ordinal>=?3 AND format=?5 ORDER BY ordinal LIMIT ?4
