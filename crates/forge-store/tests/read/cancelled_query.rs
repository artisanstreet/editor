#[test]
fn continuing_a_stopped_run_reads_its_cancelled_items_by_index_among_one_hundred_thousand_historical_items()
 {
    let mut conn = rusqlite::Connection::open_in_memory().unwrap();
    crate::schema::migrate(
        &mut conn,
        &crate::schema::Upgrade::of(&crate::Environment::system()),
    )
    .unwrap();
    conn.execute_batch(include_str!("historical_items.sql"))
        .unwrap();
    conn.execute(include_str!("cancelled_items.sql"), [])
        .unwrap();
    conn.execute_batch(include_str!("analyze.sql")).unwrap();
    let mut explain = conn.prepare(include_str!("explain_cancelled.sql")).unwrap();
    let details = explain
        .query_map(["stopped"], |row| row.get::<_, String>(3))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert!(
        details
            .iter()
            .any(|detail| detail.contains("items_cancelled_by_run")),
        "the continue prompt must not scan every item to find one run's cancelled work: {details:?}"
    );
    assert!(
        !details.iter().any(|detail| detail.contains("TEMP B-TREE")),
        "a stopped run's cancelled items must come out in transcript order without sorting: {details:?}"
    );
}
