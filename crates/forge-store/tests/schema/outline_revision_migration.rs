use super::*;

#[test]
fn stored_outlines_start_at_the_revision_of_their_latest_prompt() {
    let mut conn = Connection::open_in_memory().unwrap();
    migrate_to(&mut conn, 33, &Upgrade::of(&crate::Environment::system())).unwrap();
    let rows: [(&str, &str, u64, u64, Option<&str>); 4] = [
        ("a1", "a", 1, 7, Some("First")),
        ("a2", "a", 2, 3, Some("Second")),
        ("a3", "a", 3, 9, None),
        ("b1", "b", 4, 5, None),
    ];
    for (id, thread, position, seq, outline) in rows {
        conn.execute(
            include_str!("outline_revision_migration/insert_outlined_item.sql"),
            rusqlite::params![id, thread, position, seq, outline],
        )
        .unwrap();
    }
    migrate(&mut conn, &Upgrade::of(&crate::Environment::system())).unwrap();
    let revisions: Vec<(String, u64)> = conn
        .prepare(include_str!("outline_revision_migration/revisions.sql"))
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(revisions, [("a".to_owned(), 7)]);
}
