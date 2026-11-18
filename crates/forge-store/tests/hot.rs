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

fn changed(thread: &ThreadId) -> Envelope {
    Envelope {
        seq: None,
        live_revision: 0,
        event: Event::ThreadRemoved {
            thread_id: thread.clone(),
        },
    }
}

#[test]
fn a_long_running_forge_forgets_threads_it_no_longer_holds() {
    let hot = HotPages::default();
    for n in 0..100_000 {
        hot.committed(&[changed(&ThreadId(format!("thread-{n}")))]);
    }
    assert!(
        hot.threads_remembered() <= 4096,
        "{} threads remembered",
        hot.threads_remembered()
    );
}

#[test]
fn a_page_read_before_a_forgotten_change_is_not_served() {
    let hot = HotPages::default();
    let thread = ThreadId("raced".into());
    let tail = Tail::Rows { limit: None };
    let Lookup::Miss(read_at) = hot.lookup(&thread, &tail) else {
        panic!("an unread thread has nothing cached");
    };
    hot.committed(&[changed(&thread)]);
    for n in 0..10_000 {
        hot.committed(&[changed(&ThreadId(format!("other-{n}")))]);
    }
    hot.hold(&thread, tail.clone(), read_at, &empty_page());
    assert!(
        matches!(hot.lookup(&thread, &tail), Lookup::Miss(_)),
        "the transcript read before its change must be read again"
    );
}

#[test]
fn held_pages_of_forgotten_threads_release_their_bytes() {
    let hot = HotPages::default();
    let tail = Tail::Rows { limit: None };
    for n in 0..10_000 {
        let thread = ThreadId(format!("held-{n}"));
        let Lookup::Miss(read_at) = hot.lookup(&thread, &tail) else {
            panic!("an unread thread has nothing cached");
        };
        hot.hold(&thread, tail.clone(), read_at, &empty_page());
    }
    assert!(hot.threads_remembered() <= 4096);
    let page = serde_json::to_vec(&empty_page()).unwrap().len();
    assert_eq!(hot.held_bytes(), hot.threads_remembered() * page);
}
