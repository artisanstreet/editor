use super::{MentionRecheck, MentionSources, TITLE_RECHECK};
use artisan_api::{Thread, ThreadId};
use std::time::{Duration, Instant};
use test_support::editor::core::StubbedHost;
use test_support::editor::forge::empty_state;

fn thread(id: &str, title: &str) -> Thread {
    serde_json::from_value(serde_json::json!({
        "id": id,
        "project_id": "project",
        "title": title,
        "config": {"engine": "claude"},
        "activity_seq": 1,
        "read_seq": 1,
        "seq": 1,
        "created_at": 0,
        "updated_at": 0,
        "archived": false,
    }))
    .expect("thread")
}

fn titled(stub: &mut StubbedHost, id: &str, title: &str) {
    stub.host_mut()
        .threads
        .insert(ThreadId(id.into()), thread(id, title));
}

fn names(sources: &MentionSources, text: &str) -> Vec<String> {
    sources
        .vocabulary()
        .mentions(text)
        .into_iter()
        .filter_map(|mention| text.get(mention.range).map(str::to_owned))
        .collect()
}

#[test]
fn a_rename_inside_the_recheck_window_shows_once_the_window_ends() {
    let mut stub = StubbedHost::start(empty_state(), axum::Router::new());
    let own = ThreadId("own".into());
    titled(&mut stub, "other", "Sidebar lag");
    let mut sources = MentionSources::default();
    let start = Instant::now();
    assert!(sources.sync(stub.host(), &own, start));
    assert_eq!(names(&sources, "ask @Sidebar lag"), ["@Sidebar lag"]);

    titled(&mut stub, "other", "Sidebar fix");
    let halfway = start + Duration::from_millis(500);
    assert!(!sources.sync(stub.host(), &own, halfway));
    assert_eq!(sources.recheck_at(), Some(start + TITLE_RECHECK));

    assert!(sources.sync(stub.host(), &own, start + TITLE_RECHECK));
    assert_eq!(sources.recheck_at(), None);
    assert_eq!(names(&sources, "ask @Sidebar fix"), ["@Sidebar fix"]);
    assert!(names(&sources, "ask @Sidebar lag").is_empty());
}

#[test]
fn a_new_thread_or_another_open_thread_is_picked_up_at_once() {
    let mut stub = StubbedHost::start(empty_state(), axum::Router::new());
    let own = ThreadId("own".into());
    titled(&mut stub, "own", "Own thread");
    let mut sources = MentionSources::default();
    let start = Instant::now();
    assert!(!sources.sync(stub.host(), &own, start));
    titled(&mut stub, "new", "Fresh thread");
    let soon = start + Duration::from_millis(10);
    assert!(sources.sync(stub.host(), &own, soon));
    assert_eq!(names(&sources, "@Fresh thread"), ["@Fresh thread"]);
    assert!(sources.sync(stub.host(), &ThreadId("new".into()), soon));
    assert_eq!(names(&sources, "@Own thread"), ["@Own thread"]);
    assert!(names(&sources, "@Fresh thread").is_empty());
}

#[test]
fn an_unchanged_host_or_a_rename_undone_inside_the_window_arms_no_recheck() {
    let mut stub = StubbedHost::start(empty_state(), axum::Router::new());
    let own = ThreadId("own".into());
    titled(&mut stub, "other", "Sidebar lag");
    let mut sources = MentionSources::default();
    let start = Instant::now();
    assert!(sources.sync(stub.host(), &own, start));

    let soon = start + Duration::from_millis(100);
    assert!(!sources.sync(stub.host(), &own, soon));
    assert_eq!(sources.recheck_at(), None);

    titled(&mut stub, "other", "Sidebar fix");
    assert!(!sources.sync(stub.host(), &own, soon));
    assert_eq!(sources.recheck_at(), Some(start + TITLE_RECHECK));

    titled(&mut stub, "other", "Sidebar lag");
    let later = start + Duration::from_millis(500);
    assert!(!sources.sync(stub.host(), &own, later));
    assert_eq!(sources.recheck_at(), None);
}

fn advance(timers: &tokio::runtime::Runtime, by: Duration) {
    timers.block_on(async {
        tokio::time::sleep(by).await;
        tokio::task::yield_now().await;
    });
}

#[test]
fn the_recheck_wakes_once_when_the_paused_clock_reaches_its_due_time() {
    let timers = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .expect("a paused timer runtime");
    let start = Instant::now();
    let due = start + TITLE_RECHECK;
    let mut recheck = MentionRecheck::on(timers.handle().clone());
    recheck.arm(Some(due), start + Duration::from_millis(500));
    advance(&timers, Duration::from_millis(499));
    assert!(!recheck.fire());
    advance(&timers, Duration::from_millis(1));
    assert!(recheck.fire());
    assert!(!recheck.fire());

    recheck.arm(Some(due), start);
    recheck.arm(None, start);
    advance(&timers, TITLE_RECHECK * 2);
    assert!(!recheck.fire());
}
