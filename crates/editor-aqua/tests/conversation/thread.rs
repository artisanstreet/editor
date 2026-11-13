use artisan_api::{Item, ItemId, ItemKind, ItemsPage, Seq, Status, ThreadId};
use editor_aqua::conversation::thread::ThreadView;
use editor_aqua::conversation::viewport::{Effect, Fetch, Input};
use std::ops::RangeInclusive;

fn item(position: Seq, seq: u64) -> Item {
    Item {
        id: ItemId(format!("item-{position}")),
        thread_id: ThreadId("thread".into()),
        position,
        seq,
        run_id: None,
        parent_id: None,
        status: Status::Done,
        created_at: 0,
        updated_at: 0,
        kind: ItemKind::UserMessage {
            text: "text".into(),
            images: Vec::new(),
            steer: false,
            from_agent: None,
            sender: None,
        },
    }
}

fn page(positions: RangeInclusive<Seq>, seq: u64, more: bool) -> ItemsPage {
    ItemsPage {
        items: positions.map(|position| item(position, seq)).collect(),
        runs: Vec::new(),
        more,
        work: Vec::new(),
    }
}

fn send(view: &mut ThreadView, input: &Input) -> Vec<Effect> {
    let mut effects = Vec::new();
    view.viewport.handle_with_context(input, &mut effects);
    effects
}

fn positions(view: &ThreadView) -> Vec<Seq> {
    view.items.keys().copied().collect()
}

fn history() -> ThreadView {
    let (mut view, _) = ThreadView::open(ThreadId("thread".into()));
    view.apply_page(Fetch::Tail, page(11..=20, 1, true));
    assert_eq!(
        send(&mut view, &Input::TopReached),
        [Effect::Fetch(Fetch::Older { before: 11 })]
    );
    view.apply_page(Fetch::Older { before: 11 }, page(1..=10, 1, false));
    view
}

#[test]
fn a_refresh_keeps_older_history_and_replaces_the_tail_it_overlaps() {
    let mut view = history();
    assert_eq!(
        send(&mut view, &Input::Resume),
        [Effect::Fetch(Fetch::Refresh)]
    );

    view.apply_page(Fetch::Refresh, page(15..=22, 2, true));

    assert_eq!(positions(&view), (1..=22).collect::<Vec<_>>());
    assert_eq!(view.item(&ItemId("item-14".into())).unwrap().seq, 1);
    assert_eq!(view.item(&ItemId("item-16".into())).unwrap().seq, 2);
    assert_eq!(view.viewport.oldest, Some(1));
    assert!(view.viewport.exhausted);
}

#[test]
fn a_refresh_that_skips_past_the_parked_items_starts_over_from_the_tail() {
    let mut view = history();
    send(&mut view, &Input::Resume);

    view.apply_page(Fetch::Refresh, page(31..=40, 1, true));

    assert_eq!(positions(&view), (31..=40).collect::<Vec<_>>());
    assert_eq!(view.viewport.oldest, Some(31));
    assert!(!view.viewport.exhausted);
}
