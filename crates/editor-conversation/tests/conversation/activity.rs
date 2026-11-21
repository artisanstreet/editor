use super::line;
use artisan_api::{
    Collection, CollectionRef, ContentId, ContentRef, Item, ItemId, ItemKind, Status, ThreadId,
    ToolKind,
};
use std::collections::HashMap;

fn stored_edit(count: u64) -> Item {
    Item {
        id: ItemId("edit".into()),
        thread_id: ThreadId("thread".into()),
        position: 1,
        seq: 1,
        run_id: None,
        parent_id: None,
        status: Status::Done,
        created_at: 0,
        updated_at: 0,
        kind: ItemKind::Tool {
            tool: ToolKind::Edit {
                changes: Collection::Stored(CollectionRef {
                    source: ContentRef {
                        id: ContentId("a".repeat(64)),
                        bytes: 100,
                        lines: 2,
                    },
                    count,
                }),
            },
            title: None,
            output: None,
            exit_code: None,
            approval: None,
        },
    }
}

#[test]
fn an_edit_stored_apart_counts_its_files_in_singular_and_plural() {
    let object = |count| line(&stored_edit(count), &HashMap::new()).object;
    assert_eq!(object(1), "1 file");
    assert_eq!(object(3), "3 files");
}
