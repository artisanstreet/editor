#![cfg(feature = "qualification")]

use artisan_api::{Item, ItemsPage, ThreadId, WorkSpan};
use editor_aqua::conversation::fold::{expanded, fold};
use editor_aqua::conversation::thread::ThreadView;
use editor_aqua::conversation::viewport::Fetch;
use editor_aqua::qualification::fixture::ImmutableFixture;
use std::collections::HashSet;
use std::path::Path;

fn tool_thread() -> ThreadView {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/qualification/fixtures/capture-tool.json");
    let fixture = ImmutableFixture::read(&path).unwrap();
    let thread = ThreadId("thread-content".into());
    let (mut view, _) = ThreadView::open(thread.clone());
    let mut items = Vec::new();
    let mut work = Vec::new();
    for (index, item) in fixture
        .fixture()
        .items
        .iter()
        .filter(|item| item.thread_id == thread)
        .enumerate()
    {
        let position = u64::try_from(index).unwrap() + 1;
        let item = Item {
            id: item.id.clone(),
            thread_id: item.thread_id.clone(),
            position,
            seq: position,
            run_id: item.run_id.clone(),
            parent_id: item.parent_id.clone(),
            status: item.status,
            created_at: 0,
            updated_at: 0,
            kind: item.kind.clone(),
        };
        if item.folds_into_work() {
            work.push(WorkSpan {
                run_id: item.run_id.clone(),
                first: position,
                last: position,
                count: 1,
                worked: true,
            });
        } else {
            items.push(item);
        }
    }
    view.apply_page(
        Fetch::Tail,
        ItemsPage {
            items,
            runs: Vec::new(),
            more: false,
            work,
        },
    );
    view
}

#[test]
fn shell_tool_thread_folds_into_unique_row_keys() {
    let view = tool_thread();
    let folded = fold(&view, None, None);
    let rows = expanded(&view, folded.rows, &folded.runs);
    let keys: Vec<_> = rows
        .iter()
        .map(|(row, _)| format!("{:?}", row.key()))
        .collect();
    assert!(keys.len() > 2, "{keys:#?}");
    let unique: HashSet<_> = keys.iter().collect();
    assert_eq!(unique.len(), keys.len(), "{keys:#?}");
}
