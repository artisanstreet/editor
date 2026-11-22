use super::*;

#[test]
fn stored_diffs_keep_their_totals_after_the_counts_leave_the_content_table() {
    let mut conn = Connection::open_in_memory().unwrap();
    migrate_to(&mut conn, 31, &Upgrade::of(&crate::Environment::system())).unwrap();
    let diff = format!("@@ -1 +1 @@\n{}", "-old\n+new\n".repeat(4000));
    let mut text = artisan_api::TextContent::Inline(diff);
    crate::content::normalize_text(
        &crate::content::Stamped {
            conn: &conn,
            now: 1,
        },
        &mut text,
    )
    .unwrap();
    let artisan_api::TextContent::Stored(source) = text else {
        panic!("the diff was stored as a source");
    };
    migrate(&mut conn, &Upgrade::of(&crate::Environment::system())).unwrap();
    let columns: Vec<String> = conn
        .prepare(include_str!("diff_totals_migration/content_columns.sql"))
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(columns, ["id", "bytes", "lines", "unreferenced_at"]);
    let tool = artisan_api::ToolKind::Edit {
        changes: vec![artisan_api::FileChange {
            path: "a".into(),
            op: artisan_api::FileOp::Modify,
            diff: Some(artisan_api::TextContent::Stored(source)),
        }]
        .into(),
    };
    assert_eq!(
        crate::content::edit_totals(&conn, &tool).unwrap(),
        Some(artisan_api::EditTotals {
            added: 4000,
            removed: 4000
        })
    );
}
