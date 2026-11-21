use artisan_api::{Item, ItemId, ItemKind, ItemsPage, Phase, RunId, Status, ThreadId, WorkSpan};
use divan::Bencher;
use editor_conversation::thread::ThreadView;
use editor_conversation::viewport::Fetch;

const SPANS: u64 = 3_000;

fn main() {
    divan::main();
}

fn collapsed_work() -> ItemsPage {
    let work = (1..=SPANS)
        .map(|first| WorkSpan {
            run_id: Some(RunId(format!("run-{first}"))),
            first,
            last: first,
            count: 1,
            worked: true,
        })
        .collect();
    let reply = Item {
        id: ItemId("reply".into()),
        thread_id: ThreadId("thread".into()),
        position: SPANS + 1,
        seq: SPANS + 1,
        run_id: Some(RunId(format!("run-{SPANS}"))),
        parent_id: None,
        status: Status::Done,
        created_at: 0,
        updated_at: 0,
        kind: ItemKind::AssistantMessage {
            text: "Done.".into(),
            phase: Phase::Final,
        },
    };
    ItemsPage {
        items: vec![reply],
        runs: Vec::new(),
        more: true,
        work,
    }
}

#[divan::bench(sample_count = 20)]
fn apply_a_page_of_collapsed_work(bencher: Bencher<'_, '_>) {
    bencher
        .with_inputs(|| {
            (
                ThreadView::open(ThreadId("thread".into())).0,
                collapsed_work(),
            )
        })
        .bench_local_values(|(mut view, page)| {
            view.apply_page(Fetch::Tail, page);
            view
        });
}
