SELECT watch.body FROM coordination_records watch
LEFT JOIN coordination_records work ON work.id='work:'||json_extract(watch.body,'$.content.target')
WHERE watch.kind='watch' AND (
    json_extract(watch.body,'$.content.state')='ready'
    OR (json_extract(watch.body,'$.content.state')='pending'
        AND json_extract(work.body,'$.content.state') IN ('completed','failed','cancelled')
        AND work.revision>json_extract(watch.body,'$.content.after_revision')))
ORDER BY watch.updated_at,watch.id LIMIT 100;
