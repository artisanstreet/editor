SELECT body FROM coordination_records WHERE kind='message'
AND json_extract(body,'$.content.state') IN ('acknowledged','resolved','declined')
AND COALESCE(json_extract(body,'$.content.receipt_revision'),0)<revision
ORDER BY updated_at,id LIMIT 100;
