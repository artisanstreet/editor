#[test]
fn waiting_query_uses_its_ordered_partial_index_with_one_hundred_thousand_historical_items() {
    let mut conn = rusqlite::Connection::open_in_memory().unwrap();
    crate::schema::migrate(
        &mut conn,
        &crate::schema::Upgrade::of(&crate::Environment::system()),
    )
    .unwrap();
    conn.execute_batch(include_str!("historical_items.sql"))
        .unwrap();
    conn.execute(include_str!("waiting_items.sql"), []).unwrap();
    conn.execute_batch(include_str!("analyze.sql")).unwrap();
    let mut explain = conn.prepare(include_str!("explain_waiting.sql")).unwrap();
    let details = explain
        .query_map([], |row| row.get::<_, String>(3))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert!(
        details
            .iter()
            .any(|detail| detail.contains("items_awaiting_position")),
        "the production waiting query must use its tiny ordered index: {details:?}"
    );
    assert!(
        !details.iter().any(|detail| detail.contains("TEMP B-TREE")),
        "waiting identities must not sort historical Items: {details:?}"
    );
    let pending = super::items_awaiting_user(&conn).unwrap();
    assert_eq!(
        pending
            .iter()
            .map(|item| item.id.0.as_str())
            .collect::<Vec<_>>(),
        ["earlier", "later"]
    );
}
