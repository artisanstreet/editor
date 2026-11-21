use super::{Fetch, ThreadView, WINDOW_MAX};
use artisan_api::{Item, ItemId, ItemKind, ItemsPage, Phase, RunId, Status, ThreadId, WorkSpan};

#[test]
fn a_page_of_collapsed_work_stays_within_the_transcript_window() {
    let (mut view, _) = ThreadView::open(ThreadId("thread".into()));
    let spans = 3_000;
    let work = (1..=spans)
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
        position: spans + 1,
        seq: spans + 1,
        run_id: Some(RunId(format!("run-{spans}"))),
        parent_id: None,
        status: Status::Done,
        created_at: 0,
        updated_at: 0,
        kind: ItemKind::AssistantMessage {
            text: "Done.".into(),
            phase: Phase::Final,
        },
    };
    view.apply_page(
        Fetch::Tail,
        ItemsPage {
            items: vec![reply],
            runs: Vec::new(),
            more: true,
            work,
        },
    );
    assert!(
        view.items.len() + view.work.len() <= WINDOW_MAX,
        "{} spans kept",
        view.work.len()
    );
    assert!(view.item(&ItemId("reply".into())).is_some());
    assert_eq!(view.work.keys().next_back(), Some(&spans));
}
