use artisan_api::{ThreadCursor, ThreadId, ThreadWindow, ThreadsQuery};
use serde_json::json;

#[test]
fn a_thread_page_query_names_one_direction_with_a_whole_cursor() {
    let query: ThreadsQuery = serde_json::from_value(json!({
        "archived": true,
        "after_activity": 7,
        "after_id": "t1",
        "limit": 20,
    }))
    .expect("a newer page");
    assert_eq!(
        query.window,
        ThreadWindow::After(ThreadCursor {
            activity_seq: 7,
            id: ThreadId::from("t1"),
        })
    );
    assert_eq!(
        serde_json::from_value::<ThreadsQuery>(serde_json::to_value(&query).expect("encode"))
            .expect("decode"),
        query
    );
    assert_eq!(
        serde_json::from_value::<ThreadsQuery>(json!({}))
            .expect("the newest page")
            .window,
        ThreadWindow::Latest
    );
}

#[test]
fn half_cursors_and_two_directions_are_refused_at_the_boundary() {
    for malformed in [
        json!({ "before_activity": 7 }),
        json!({ "after_id": "t1" }),
        json!({
            "before_activity": 7,
            "before_id": "t1",
            "after_activity": 9,
            "after_id": "t2",
        }),
    ] {
        serde_json::from_value::<ThreadsQuery>(malformed.clone())
            .expect_err("a malformed cursor is refused");
    }
}
