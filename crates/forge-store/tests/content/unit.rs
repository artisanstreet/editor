use super::*;
fn database() -> Connection {
    let mut conn = Connection::open_in_memory().unwrap();
    crate::schema::migrate(&mut conn).unwrap();
    conn
}
fn stored(conn: &Connection, text: &str) -> ContentRef {
    let mut value = TextContent::Inline(text.to_owned());
    normalize_text(conn, &mut value).unwrap();
    let TextContent::Stored(source) = value else {
        panic!("large payload was not externalized")
    };
    source
}
#[test]
fn ranges_reconstruct_multimegabyte_unicode_without_item_reads() {
    let conn = database();
    let text = "αβ🙂\nsource\0".repeat(800_000);
    let source = stored(&conn, &text);
    let again = stored(&conn, &text);
    assert_eq!(source, again);
    assert_eq!(
        conn.query_row(include_str!("content_rows.sql"), [], |row| row
            .get::<_, u64>(0))
            .unwrap(),
        1
    );
    let mut rebuilt = String::new();
    let mut offset = 0;
    while offset < source.bytes {
        let part = range(
            &conn,
            &source.id,
            &ContentQuery {
                offset,
                limit: Some(65536),
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(part.start, offset);
        assert!(part.text.len() <= 65536);
        assert_eq!(part.end - part.start, part.text.len() as u64);
        offset = part.end;
        rebuilt.push_str(&part.text);
    }
    assert_eq!(rebuilt, text);
    for offset in [1, 3, 5, 32767, 32768, 65535] {
        let part = range(
            &conn,
            &source.id,
            &ContentQuery {
                offset,
                limit: Some(19),
            },
        )
        .unwrap()
        .unwrap();
        assert!(part.start <= offset && offset - part.start <= 3);
        assert_eq!(
            Some(part.text.as_str()),
            text.get(usize::try_from(part.start).unwrap()..usize::try_from(part.end).unwrap())
        );
    }
    range(
        &conn,
        &source.id,
        &ContentQuery {
            offset: source.bytes + 1,
            limit: None,
        },
    )
    .expect_err("an offset past the end is refused");
    range(
        &conn,
        &source.id,
        &ContentQuery {
            offset: 0,
            limit: Some(65537),
        },
    )
    .expect_err("a limit over the range cap is refused");
}
#[test]
fn transaction_rollback_does_not_leave_chunks_and_gc_keeps_owned_sources() {
    let mut conn = database();
    {
        let tx = conn.transaction().unwrap();
        stored(&tx, &"ab".repeat(20_000));
    }
    assert_eq!(
        conn.query_row(include_str!("chunk_rows.sql"), [], |row| row
            .get::<_, u64>(0))
            .unwrap(),
        0
    );
    let source = stored(&conn, &"payload".repeat(4_000));
    conn.execute(include_str!("own_by_owner.sql"), [source.id.as_str()])
        .unwrap();
    assert_eq!(collect(&conn, i64::MAX).unwrap(), 0);
    conn.execute(include_str!("disown_owner.sql"), []).unwrap();
    assert_eq!(collect(&conn, artisan_api::now()).unwrap(), 0);
    assert_eq!(
        collect(&conn, artisan_api::now() + GRACE_MILLIS + 1000).unwrap(),
        1
    );
    assert_eq!(
        conn.query_row(include_str!("chunk_rows.sql"), [], |row| row
            .get::<_, u64>(0))
            .unwrap(),
        0
    );
}
#[test]
fn stored_edit_counts_are_not_zeroed_when_status_changes() {
    let conn = database();
    let diff = format!(
        "--- old\n+++ new\n@@ -1,1 +1,1 @@\n{}",
        "-old\n+new\n".repeat(3000)
    );
    let expected = diff_totals(&diff);
    let source = stored(&conn, &diff);
    let tool = ToolKind::Edit {
        changes: vec![artisan_api::FileChange {
            path: "a".into(),
            op: artisan_api::FileOp::Modify,
            diff: Some(TextContent::Stored(source)),
        }]
        .into(),
    };
    assert_eq!(inline_edit_totals(&tool), None);
    assert_eq!(edit_totals(&conn, &tool).unwrap(), Some(expected));
}
#[test]
fn unknown_and_inconsistent_references_are_rejected_and_chunk_gaps_are_errors() {
    let conn = database();
    let source = stored(&conn, &"abcd".repeat(25000));
    let mut known = TextContent::Stored(source.clone());
    normalize_text(&conn, &mut known).unwrap();
    let mut wrong_length = source.clone();
    wrong_length.bytes += 1;
    normalize_text(&conn, &mut TextContent::Stored(wrong_length))
        .expect_err("a reference with the wrong length is refused");
    let mut unknown = source.clone();
    unknown.id = ContentId("f".repeat(64));
    normalize_text(&conn, &mut TextContent::Stored(unknown))
        .expect_err("a reference to unknown content is refused");
    conn.execute(include_str!("drop_second_chunk.sql"), [source.id.as_str()])
        .unwrap();
    range(
        &conn,
        &source.id,
        &ContentQuery {
            offset: 0,
            limit: None,
        },
    )
    .expect_err("a range over a missing chunk is refused");
}
#[test]
fn collection_batches_are_bounded_and_make_progress_on_a_large_source() {
    let conn = database();
    for (id, bytes) in [
        ("a", 20 * 1024 * 1024),
        ("b", 10 * 1024 * 1024),
        ("c", 10 * 1024 * 1024),
    ] {
        conn.execute(include_str!("insert_inconsistent.sql"), params![id, bytes])
            .unwrap();
    }
    assert_eq!(collect(&conn, i64::MAX).unwrap(), 1);
    assert_eq!(collect(&conn, i64::MAX).unwrap(), 1);
    assert_eq!(collect(&conn, i64::MAX).unwrap(), 1);
    assert_eq!(collect(&conn, i64::MAX).unwrap(), 0);
}
