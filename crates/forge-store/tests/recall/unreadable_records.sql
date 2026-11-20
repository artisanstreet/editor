UPDATE content_chunks SET data = CAST(replace(CAST(data AS TEXT), '"path":', '"pxth":') AS BLOB)
