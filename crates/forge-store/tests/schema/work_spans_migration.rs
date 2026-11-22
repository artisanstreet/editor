use super::*;
use artisan_api::{RunId, ThreadId, WorkSpan};

struct Root {
    id: &'static str,
    thread: &'static str,
    position: u64,
    kind: &'static str,
    parent: Option<&'static str>,
    work: bool,
    run: Option<&'static str>,
}

const fn root(
    id: &'static str,
    thread: &'static str,
    position: u64,
    kind: &'static str,
    parent: Option<&'static str>,
    work: bool,
    run: Option<&'static str>,
) -> Root {
    Root {
        id,
        thread,
        position,
        kind,
        parent,
        work,
        run,
    }
}

#[test]
fn stored_threads_get_their_collapsed_work_when_spans_are_first_kept() {
    let mut conn = Connection::open_in_memory().unwrap();
    migrate_to(&mut conn, 32, &Upgrade::of(&crate::Environment::system())).unwrap();
    let rows = [
        root("p1", "a", 1, "user_message", None, false, None),
        root("t1", "a", 2, "tool", None, true, Some("r1")),
        root("c1", "a", 3, "tool", Some("t1"), true, Some("r2")),
        root("m1", "a", 4, "assistant_message", None, true, Some("r1")),
        root("t2", "a", 5, "tool", None, true, Some("r2")),
        root("f1", "a", 6, "assistant_message", None, false, Some("r2")),
        root("m2", "a", 7, "assistant_message", None, true, Some("r2")),
        root("b1", "b", 8, "agent", None, true, None),
        root("b2", "b", 9, "tool", None, true, None),
    ];
    for row in rows {
        conn.execute(
            include_str!("work_spans_migration/insert_root_item.sql"),
            rusqlite::params![
                row.id,
                row.thread,
                row.position,
                row.kind,
                row.parent,
                row.work,
                row.run
            ],
        )
        .unwrap();
    }
    migrate(&mut conn, &Upgrade::of(&crate::Environment::system())).unwrap();
    let span = |run: Option<&str>, first, last, count, worked| WorkSpan {
        run_id: run.map(RunId::from),
        first,
        last,
        count,
        worked,
    };
    assert_eq!(
        crate::work_spans::between(&conn, &ThreadId::from("a"), 0, i64::MAX).unwrap(),
        [
            span(Some("r1"), 2, 4, 2, true),
            span(Some("r2"), 5, 5, 1, true),
            span(Some("r2"), 7, 7, 1, false),
        ]
    );
    assert_eq!(
        crate::work_spans::between(&conn, &ThreadId::from("b"), 0, i64::MAX).unwrap(),
        [span(None, 8, 9, 2, true)]
    );
}
