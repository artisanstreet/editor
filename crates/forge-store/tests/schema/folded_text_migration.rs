use super::*;
use artisan_api::TextSearchQuery;
use serde_json::json;

fn found(conn: &Connection, query: &str) -> Vec<(String, String)> {
    crate::text_search::text_hits(
        conn,
        &TextSearchQuery {
            query: query.into(),
            limit: None,
        },
    )
    .expect("search the migrated messages")
    .into_iter()
    .map(|hit| (hit.item_id.0, hit.snippet))
    .collect()
}

#[test]
fn messages_stored_before_folding_are_found_regardless_of_unicode_case() {
    let mut conn = Connection::open_in_memory().expect("open the database");
    migrate_to(&mut conn, 29).expect("migrate to the schema before folded text");
    conn.execute(
        include_str!("insert_thread.sql"),
        rusqlite::params!["t", json!({"title": "Old", "project_id": "p"}).to_string()],
    )
    .expect("insert the thread");
    for (position, (id, text)) in [
        ("butter", "Brød og SMØR til frokost"),
        ("street", "Die Straße ist lang"),
        ("cafe", "Kaffe på CAFE\u{301}EN"),
    ]
    .into_iter()
    .enumerate()
    {
        let body = json!({"id": id, "thread_id": "t", "position": position, "seq": position, "run_id": null, "parent_id": null, "status": "done", "created_at": 1, "updated_at": 1, "kind": {"type": "assistant_message", "text": text, "phase": "final"}});
        conn.execute(
            include_str!("insert_typed_item.sql"),
            rusqlite::params![id, position, "done", body.to_string()],
        )
        .expect("insert the message");
        conn.execute(include_str!("insert_item_text.sql"), [id, text])
            .expect("index the message the old way");
    }

    migrate(&mut conn).expect("migrate to today's schema");
    crate::recall::refresh_index(&mut conn).expect("rebuild the recall index");

    assert_eq!(
        found(&conn, "smør"),
        [("butter".into(), "Brød og SMØR til frokost".into())]
    );
    assert_eq!(
        found(&conn, "STRASSE"),
        [("street".into(), "Die Straße ist lang".into())]
    );
    assert_eq!(
        found(&conn, "cafe"),
        Vec::<(String, String)>::new(),
        "only whole accented letters match"
    );
    assert_eq!(
        found(&conn, "Caféen"),
        [("cafe".into(), "Kaffe på CAFE\u{301}EN".into())]
    );
}
