use super::*;
use serde_json::json;

#[test]
fn activity_backfill_uses_source_events_and_preserves_revisions() {
    let mut conn = Connection::open_in_memory().unwrap();
    migrate_to(&mut conn, 11, Upgrade::of(&crate::Environment::system())).unwrap();
    for (id, activity) in [
        ("edited-input", 20),
        ("delivered-input", 30),
        ("run", 40),
        ("older-run", 60),
        ("missing-event", 70),
    ] {
        conn.execute(
            include_str!("insert_thread.sql"),
            rusqlite::params![
                id,
                json!({"activity_seq":activity,"created_at":10,"updated_at":9999}).to_string()
            ],
        )
        .unwrap();
    }
    conn.execute(include_str!("insert_edited_input.sql"),
        [json!({"id":"a","thread_id":"edited-input","position":15,"seq":20,"run_id":null,"parent_id":null,"status":"done","created_at":100,"updated_at":200,"kind":{"type":"user_message","text":"edited input","images":[],"steer":false}}).to_string()]).unwrap();
    conn.execute(include_str!("insert_delivered_input.sql"),
        [json!({"id":"b","thread_id":"delivered-input","position":30,"seq":31,"run_id":null,"parent_id":null,"status":"done","created_at":300,"updated_at":310,"kind":{"type":"user_message","text":"delivered input","images":[],"steer":false}}).to_string()]).unwrap();
    conn.execute(
        include_str!("insert_run.sql"),
        [json!({"started_at":350,"ended_at":400}).to_string()],
    )
    .unwrap();
    conn.execute(
        include_str!("insert_older_run.sql"),
        [json!({"started_at":450,"ended_at":500}).to_string()],
    )
    .unwrap();
    conn.execute(
        include_str!("insert_newer_run.sql"),
        [json!({"started_at":600,"ended_at":650}).to_string()],
    )
    .unwrap();
    migrate(&mut conn, Upgrade::of(&crate::Environment::system())).unwrap();
    for (id, expected) in [
        ("edited-input", 200),
        ("delivered-input", 300),
        ("run", 400),
        ("older-run", 500),
        ("missing-event", 10),
    ] {
        let (seq, activity_at, updated_at): (u64, i64, i64) = conn
            .query_row(include_str!("thread_activity.sql"), [id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .unwrap();
        assert_eq!((seq, activity_at, updated_at), (99, expected, 9999), "{id}");
    }
    migrate(&mut conn, Upgrade::of(&crate::Environment::system())).unwrap();
    assert_eq!(
        conn.pragma_query_value::<usize, _>(None, "user_version", |row| row.get(0))
            .unwrap(),
        MIGRATIONS.len()
    );
}
