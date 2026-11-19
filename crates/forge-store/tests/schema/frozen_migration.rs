use super::*;
use serde_json::json;

#[test]
fn a_finished_edit_in_a_shape_today_does_not_read_still_folds_into_its_work() {
    let mut conn = Connection::open_in_memory().unwrap();
    migrate_to(&mut conn, 21, Upgrade::of(&crate::Environment::system())).unwrap();
    let edit = json!({
        "id": "renamed",
        "thread_id": "t",
        "position": 1,
        "seq": 1,
        "run_id": "r",
        "parent_id": null,
        "status": "done",
        "created_at": 1,
        "updated_at": 1,
        "kind": {
            "type": "tool",
            "tool": {
                "kind": "edit",
                "changes": [{"path": "a.rs", "op": "rename", "to": "b.rs", "diff": null}]
            }
        }
    });
    conn.execute(
        include_str!("insert_typed_item.sql"),
        rusqlite::params!["renamed", 1, "done", edit.to_string()],
    )
    .unwrap();

    migrate(&mut conn, Upgrade::of(&crate::Environment::system())).unwrap();

    let work: bool = conn
        .query_row(include_str!("item_work.sql"), ["renamed"], |row| row.get(0))
        .unwrap();
    assert!(
        work,
        "the edit folds into the work section like every other tool"
    );
}
