INSERT INTO coordination_records(id,owner,project_id,kind,revision,updated_at,body)
VALUES(?1,?2,?3,?4,?5,?6,?7)
ON CONFLICT(id) DO UPDATE SET owner=excluded.owner,project_id=excluded.project_id,
kind=excluded.kind,revision=excluded.revision,updated_at=excluded.updated_at,body=excluded.body;
