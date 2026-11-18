use artisan_api::{Envelope, Event, ItemsPage, ThreadId};
use forge_store::hot::{HotPages, Lookup, Tail};

fn empty_page() -> ItemsPage {
    ItemsPage {
        items: Vec::new(),
        runs: Vec::new(),
        more: false,
        work: Vec::new(),
    }
}

#[test]
fn a_page_read_before_a_reset_is_not_served_after_it() {
    let hot = HotPages::default();
    let thread = ThreadId("never-read-before".into());
    let tail = Tail::Rows { limit: None };
    let Lookup::Miss(read_at) = hot.lookup(&thread, &tail) else {
        panic!("an unread thread has nothing cached");
    };
    hot.committed(&[Envelope {
        seq: None,
        live_revision: 0,
        event: Event::Reset,
    }]);
    hot.hold(&thread, tail.clone(), read_at, &empty_page());
    assert!(
        matches!(hot.lookup(&thread, &tail), Lookup::Miss(_)),
        "the transcript read before the reset must be read again"
    );
}

#[test]
fn a_page_read_after_a_reset_is_served() {
    let hot = HotPages::default();
    let thread = ThreadId("fresh".into());
    let tail = Tail::Rows { limit: None };
    hot.committed(&[Envelope {
        seq: None,
        live_revision: 0,
        event: Event::Reset,
    }]);
    let Lookup::Miss(read_at) = hot.lookup(&thread, &tail) else {
        panic!("an unread thread has nothing cached");
    };
    hot.hold(&thread, tail.clone(), read_at, &empty_page());
    assert!(matches!(hot.lookup(&thread, &tail), Lookup::Hit(_)));
}
