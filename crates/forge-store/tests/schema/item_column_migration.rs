use super::*;
use artisan_api::{Item, ItemId, ItemKind, ItemsQuery, Status, ThreadId};

const WHOLE: ItemsQuery = ItemsQuery {
    before: None,
    after: None,
    limit: None,
};

fn item(id: &str, parent: Option<&str>, status: Status, kind: ItemKind) -> Item {
    Item {
        id: ItemId(id.into()),
        thread_id: ThreadId("t".into()),
        position: 0,
        seq: 0,
        run_id: None,
        parent_id: parent.map(|parent| ItemId(parent.into())),
        status,
        created_at: 1,
        updated_at: 1,
        kind,
    }
}

#[test]
fn existing_items_keep_their_queue_and_transcript_after_the_column_migration() {
    let mut conn = Connection::open_in_memory().unwrap();
    migrate_to(&mut conn, 15, Upgrade::of(&crate::Environment::system())).unwrap();
    let rows = [
        item(
            "input",
            None,
            Status::Running,
            ItemKind::UserMessage {
                text: "pending".into(),
                images: Vec::new(),
                steer: false,
                from_agent: None,
                sender: None,
            },
        ),
        item(
            "child",
            Some("agent"),
            Status::Done,
            ItemKind::Notice {
                kind: artisan_api::NoticeKind::Info,
                text: "subagent work".into(),
            },
        ),
    ];
    for (seq, row) in rows.iter().enumerate() {
        conn.execute(
            include_str!("insert_legacy_item.sql"),
            rusqlite::params![
                row.id.as_str(),
                seq + 1,
                crate::read::tag(&row.status),
                serde_json::to_string(row).unwrap()
            ],
        )
        .unwrap();
    }
    migrate(&mut conn, Upgrade::of(&crate::Environment::system())).unwrap();
    assert_eq!(
        crate::read::pending_threads(&conn, 8, &[], artisan_api::now()).unwrap(),
        [ThreadId("t".into())]
    );
    let agent = ItemId("agent".into());
    let page =
        crate::read::scoped_items_page(&conn, &ThreadId("t".into()), &WHOLE, Some(Some(&agent)))
            .unwrap();
    assert_eq!(
        page.items
            .iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>(),
        ["child"]
    );
    let root =
        crate::read::scoped_items_page(&conn, &ThreadId("t".into()), &WHOLE, Some(None)).unwrap();
    assert_eq!(
        root.items
            .iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>(),
        ["input"]
    );
}

#[test]
fn repeated_compaction_items_collapse_to_one_per_compaction() {
    use serde_json::json;
    let mut conn = Connection::open_in_memory().unwrap();
    migrate_to(&mut conn, 17, Upgrade::of(&crate::Environment::system())).unwrap();
    let compaction = |id: &str, run: &str, position: i64, before: Option<u64>| {
        let body = json!({"id":id,"thread_id":"t","position":position,"seq":position,
            "run_id":run,"parent_id":null,"status":"done","created_at":1,"updated_at":1,
            "kind":{"type":"compaction","tokens_before":before,
                "tokens_after":before.map(|_| 15_000)}});
        conn.execute(
            include_str!("insert_compaction_item.sql"),
            rusqlite::params![id, position, run, body.to_string()],
        )
        .unwrap();
    };
    compaction("progress-1", "r1", 1, None);
    compaction("progress-2", "r1", 2, None);
    compaction("boundary", "r1", 3, Some(966_000));
    compaction("interrupted-1", "r2", 4, None);
    compaction("interrupted-2", "r2", 5, None);
    compaction("other-run", "r3", 6, Some(500_000));
    migrate(&mut conn, Upgrade::of(&crate::Environment::system())).unwrap();
    let page = crate::read::scoped_items_page(&conn, &ThreadId("t".into()), &WHOLE, None).unwrap();
    let ids: Vec<&str> = page.items.iter().map(|item| item.id.as_str()).collect();
    assert_eq!(ids, ["boundary", "interrupted-1", "other-run"]);
}

#[test]
fn stored_approvals_move_onto_their_items_and_agent_messages_name_their_agent() {
    use serde_json::json;
    let mut conn = Connection::open_in_memory().unwrap();
    migrate_to(&mut conn, 16, Upgrade::of(&crate::Environment::system())).unwrap();
    let row = |id: &str, position: i64, status: &str, kind: serde_json::Value| {
        json!({"id":id,"thread_id":"t","position":position,"seq":position,"run_id":null,
            "parent_id":null,"status":status,"created_at":1,"updated_at":1,"kind":kind})
    };
    let approval = json!({"request":{"type":"command","command":"touch a","cwd":"/w"},
        "reason":null,"options":["allow_once","deny_once"],"decision":"allow_once"});
    let rows = [
        row(
            "tool",
            1,
            "done",
            json!({"type":"tool","tool":{"kind":"shell","command":"touch a","cwd":"/w"},
            "title":null,"output":null,"exit_code":0,"approval":null}),
        ),
        row(
            "asked",
            2,
            "done",
            json!({"type":"approval","target":"tool","approval":approval}),
        ),
        row(
            "orphan",
            3,
            "declined",
            json!({"type":"approval","target":null,"approval":approval}),
        ),
        row(
            "agent",
            4,
            "running",
            json!({"type":"agent","name":null,"description":"task",
            "agent_id":"a935b57d5f0b8fb33","background":true,"result":null}),
        ),
        row(
            "sent",
            5,
            "done",
            json!({"type":"tool","tool":{"kind":"agent_message",
            "recipient":"a935b57d5f0b8fb33","recipient_label":"an agent","summary":null,"text":"hi"},
            "title":null,"output":null,"exit_code":null,"approval":null}),
        ),
    ];
    for row in &rows {
        conn.execute(
            include_str!("insert_typed_item.sql"),
            rusqlite::params![
                row["id"].as_str().unwrap(),
                row["position"].as_i64().unwrap(),
                row["status"].as_str().unwrap(),
                row.to_string()
            ],
        )
        .unwrap();
    }
    migrate(&mut conn, Upgrade::of(&crate::Environment::system())).unwrap();
    let page = crate::read::scoped_items_page(&conn, &ThreadId("t".into()), &WHOLE, None).unwrap();
    let ids: Vec<&str> = page.items.iter().map(|item| item.id.as_str()).collect();
    assert_eq!(ids, ["tool", "orphan", "agent", "sent"]);
    let ItemKind::Tool {
        approval: Some(moved),
        ..
    } = &page.items[0].kind
    else {
        panic!("the approval moved onto its command: {:?}", page.items[0]);
    };
    assert_eq!(moved.decision, Some(artisan_api::Decision::AllowOnce));
    assert!(matches!(
        &page.items[1].kind,
        ItemKind::Tool {
            approval: Some(_),
            ..
        }
    ));
    let agent = ItemId("agent".into());
    let scoped =
        crate::read::scoped_items_page(&conn, &ThreadId("t".into()), &WHOLE, Some(Some(&agent)))
            .unwrap();
    assert_eq!(scoped.items.len(), 1);
    assert!(matches!(
        &scoped.items[0].kind,
        ItemKind::Tool {
            tool: artisan_api::ToolKind::AgentMessage { target: Some(target), .. },
            ..
        } if *target == agent
    ));
}
