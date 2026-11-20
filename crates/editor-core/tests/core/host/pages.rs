use artisan_api::{Event, Thread, ThreadId, ThreadsPage};
use axum::http::StatusCode;
use editor_core::ThreadPageDirection;
use test_support::editor::core::StubbedHost;
use test_support::editor::forge::{HeldRequest, empty_state};

fn forge() -> StubbedHost {
    StubbedHost::start(empty_state(), axum::Router::new())
}

fn thread(id: &str, seq: u64, activity: u64, archived: bool) -> Thread {
    serde_json::from_value(serde_json::json!({
        "id": id,
        "project_id": "project",
        "title": id,
        "config": {"engine": "claude"},
        "activity_seq": activity,
        "read_seq": 0,
        "seq": seq,
        "created_at": 0,
        "updated_at": 0,
        "archived": archived,
    }))
    .expect("thread")
}

fn page(threads: Vec<Thread>) -> ThreadsPage {
    ThreadsPage {
        threads,
        runs: Vec::new(),
        next: None,
        newer: false,
    }
}

fn ids(forge: &StubbedHost, archived: bool) -> Vec<String> {
    forge
        .host()
        .thread_page(archived)
        .ids()
        .iter()
        .map(|id| id.0.clone())
        .collect()
}

fn title(forge: &StubbedHost, id: &str) -> Option<String> {
    forge
        .host()
        .threads
        .get(&ThreadId(id.into()))
        .and_then(|thread| thread.title.clone())
}

fn working(forge: &StubbedHost, thread: &str) -> bool {
    forge.host().is_working(&ThreadId(thread.into()))
}

fn start_loading(
    forge: &mut StubbedHost,
    archived: bool,
    direction: ThreadPageDirection,
) -> HeldRequest {
    forge.host_mut().load_threads(archived, direction);
    assert!(
        forge.host().thread_page(archived).is_loading(),
        "the page starts loading"
    );
    forge.forge.take("GET", "/threads")
}

fn settle(forge: &mut StubbedHost, archived: bool) {
    forge.settle("the page completion applies", |core, host| {
        core.host(host)
            .is_some_and(|data| !data.thread_page(archived).is_loading())
    });
}

fn publish_and_apply(forge: &mut StubbedHost, events: Vec<Event>) {
    for event in events {
        forge.forge.publish(event);
    }
    forge.apply_published();
}

#[test]
fn a_page_that_started_before_newer_events_does_not_roll_them_back() {
    let mut forge = forge();
    let loading = start_loading(&mut forge, false, ThreadPageDirection::Latest);
    let mut renamed = thread("renamed", 9, 9, false);
    renamed.title = Some("renamed later".into());
    publish_and_apply(
        &mut forge,
        vec![
            Event::Thread { thread: renamed },
            Event::ThreadRemoved {
                thread_id: ThreadId("deleted".into()),
            },
        ],
    );
    loading.respond(page(vec![
        thread("renamed", 3, 3, false),
        thread("deleted", 2, 2, false),
        thread("kept", 1, 1, false),
    ]));
    settle(&mut forge, false);
    assert_eq!(
        ids(&forge, false),
        ["renamed", "kept"],
        "a deleted thread stays deleted"
    );
    assert_eq!(
        title(&forge, "renamed").as_deref(),
        Some("renamed later"),
        "the page's older copy does not replace the newer event"
    );
    assert_eq!(title(&forge, "deleted"), None);
}

#[test]
fn a_superseded_page_is_ignored_and_the_current_one_applies() {
    let mut forge = forge();
    let superseded = start_loading(&mut forge, false, ThreadPageDirection::Latest);
    forge.forge.publish(Event::Reset);
    forge.settle("Forge's state reloads", |core, host| {
        core.host(host)
            .is_some_and(|data| !data.thread_page(false).is_loading())
    });
    let current = start_loading(&mut forge, false, ThreadPageDirection::Latest);
    superseded.respond(page(vec![thread("stale", 1, 1, false)]));
    current.respond(page(vec![thread("current", 1, 1, false)]));
    settle(&mut forge, false);
    assert_eq!(ids(&forge, false), ["current"]);
    assert_eq!(
        title(&forge, "stale"),
        None,
        "nothing from the superseded page is kept"
    );
}

#[test]
fn archived_threads_land_only_in_the_archive_and_older_pages_append() {
    let mut forge = forge();
    start_loading(&mut forge, false, ThreadPageDirection::Latest).respond(ThreadsPage {
        next: Some(artisan_api::ThreadCursor {
            activity_seq: 5,
            id: ThreadId("newest".into()),
        }),
        ..page(vec![
            thread("newest", 6, 6, false),
            thread("archived", 5, 5, true),
        ])
    });
    settle(&mut forge, false);
    assert_eq!(
        ids(&forge, false),
        ["newest"],
        "archived threads stay out of the inbox"
    );
    assert!(
        forge.host().thread_page(false).has_older(),
        "older threads wait until the list scrolls to them"
    );
    start_loading(&mut forge, false, ThreadPageDirection::Older)
        .respond(page(vec![thread("oldest", 1, 1, false)]));
    settle(&mut forge, false);
    assert_eq!(
        ids(&forge, false),
        ["newest", "oldest"],
        "older pages add below"
    );
    assert!(
        ids(&forge, true).is_empty(),
        "the archive page was never asked for"
    );
}

fn run(thread: &str, seq: u64, state: artisan_api::RunState) -> artisan_api::Run {
    artisan_api::Run {
        config: None,
        id: artisan_api::RunId("run".into()),
        thread_id: ThreadId(thread.into()),
        engine: artisan_api::EngineId::Claude,
        model: None,
        input_cutoff: 0,
        state,
        usage: artisan_api::Usage::default(),
        session_id: None,
        seq,
        started_at: 1,
        continuation: None,
    }
}

fn done() -> artisan_api::RunState {
    artisan_api::RunState::Finished {
        outcome: artisan_api::RunOutcome::Done,
        error: None,
        ended_at: 2,
    }
}

#[test]
fn a_page_loaded_during_a_stop_keeps_the_run_stopped() {
    let mut forge = forge();
    publish_and_apply(
        &mut forge,
        vec![
            Event::Thread {
                thread: thread("busy", 1, 1, false),
            },
            Event::Run {
                run: run("busy", 2, artisan_api::RunState::Running),
            },
        ],
    );
    forge
        .host_mut()
        .stop_optimistically(&ThreadId("busy".into()));
    assert!(
        !working(&forge, "busy"),
        "Stop shows the run stopped at once"
    );
    start_loading(&mut forge, false, ThreadPageDirection::Latest).respond(ThreadsPage {
        runs: vec![run("busy", 2, artisan_api::RunState::Running)],
        ..page(vec![thread("busy", 1, 1, false)])
    });
    settle(&mut forge, false);
    assert!(
        !working(&forge, "busy"),
        "a page that left Forge before the stop does not bring the run back"
    );
}

#[test]
fn a_page_loaded_before_a_delete_does_not_bring_back_its_running_run() {
    let mut forge = forge();
    let loading = start_loading(&mut forge, false, ThreadPageDirection::Latest);
    publish_and_apply(
        &mut forge,
        vec![Event::ThreadRemoved {
            thread_id: ThreadId("deleted".into()),
        }],
    );
    loading.respond(ThreadsPage {
        runs: vec![run("deleted", 2, artisan_api::RunState::Running)],
        ..page(vec![thread("deleted", 1, 1, false)])
    });
    settle(&mut forge, false);
    assert!(!working(&forge, "deleted"));
    assert!(
        !forge.host().runs.contains_key(&ThreadId("deleted".into())),
        "the deleted thread's run stays forgotten"
    );
}

#[test]
fn a_deleted_threads_guards_retire_once_no_older_request_can_bring_it_back() {
    let mut forge = forge();
    publish_and_apply(
        &mut forge,
        vec![
            Event::Thread {
                thread: thread("busy", 1, 1, false),
            },
            Event::Run {
                run: run("busy", 2, artisan_api::RunState::Running),
            },
        ],
    );
    forge
        .host_mut()
        .stop_optimistically(&ThreadId("busy".into()));
    publish_and_apply(
        &mut forge,
        vec![
            Event::ThreadRemoved {
                thread_id: ThreadId("busy".into()),
            },
            Event::Thread {
                thread: thread("other", 4, 4, false),
            },
        ],
    );
    assert!(title(&forge, "busy").is_none());

    publish_and_apply(
        &mut forge,
        vec![
            Event::Thread {
                thread: thread("busy", 5, 5, false),
            },
            Event::Run {
                run: run("busy", 6, artisan_api::RunState::Running),
            },
        ],
    );
    assert_eq!(
        title(&forge, "busy").as_deref(),
        Some("busy"),
        "with no request outstanding from before the delete, Forge's newer word stands"
    );
    assert!(
        working(&forge, "busy"),
        "the stop made before the delete no longer holds the run stopped"
    );
}

#[test]
fn a_queued_older_run_event_does_not_undo_a_newer_page() {
    let mut forge = forge();
    start_loading(&mut forge, false, ThreadPageDirection::Latest).respond(ThreadsPage {
        runs: vec![run("finished", 9, done())],
        ..page(vec![thread("finished", 1, 1, false)])
    });
    settle(&mut forge, false);
    publish_and_apply(
        &mut forge,
        vec![Event::Run {
            run: run("finished", 5, artisan_api::RunState::Running),
        }],
    );
    assert!(
        !working(&forge, "finished"),
        "the finished run stays finished"
    );
}

#[test]
fn scrolling_far_back_keeps_a_bounded_window_and_reloads_newer_threads() {
    let mut forge = forge();
    let mut activity = 10_000;
    let mut direction = ThreadPageDirection::Latest;
    for _ in 0..8 {
        let loading = start_loading(&mut forge, false, direction);
        let threads: Vec<Thread> = (0..200)
            .map(|_| {
                activity -= 1;
                let mut read = thread(&format!("t{activity}"), activity, activity, false);
                read.read_seq = activity;
                read
            })
            .collect();
        let last = threads.last().expect("a full page").id.clone();
        loading.respond(ThreadsPage {
            next: Some(artisan_api::ThreadCursor {
                activity_seq: activity,
                id: last,
            }),
            ..page(threads)
        });
        settle(&mut forge, false);
        direction = ThreadPageDirection::Older;
    }
    let ids = ids(&forge, false);
    assert_eq!(ids.len(), 1_000, "the list keeps five pages");
    assert_eq!(ids.first().map(String::as_str), Some("t9399"));
    assert_eq!(ids.last().map(String::as_str), Some("t8400"));
    let host = forge.host();
    assert!(
        !host.thread_page(false).at_latest(),
        "scrolling back up asks for the newer threads again"
    );
    assert!(
        host.threads.len() <= 1_000,
        "threads outside the window are released"
    );
}

fn opened(refusal: serde_json::Value) -> editor_core::ThreadUnavailable {
    let mut forge = forge();
    let thread = ThreadId("gone-thread".into());
    forge.core.navigate(editor_core::Screen::Thread {
        host: forge.host.clone(),
        thread: thread.clone(),
    });
    forge
        .forge
        .take("GET", "/threads/gone-thread")
        .respond_with(StatusCode::NOT_FOUND, refusal);
    let mut unavailable = None;
    forge.settle("the thread lookup settles", |core, host| {
        unavailable = core
            .host(host)
            .and_then(|data| data.thread_unavailable(&thread))
            .cloned();
        unavailable.is_some()
    });
    unavailable.expect("the lookup settled")
}

#[test]
fn a_thread_forge_says_is_missing_shows_as_missing_without_a_retry() {
    let refusal = artisan_api::ApiError::new(
        artisan_api::Failure::Thread(artisan_api::ThreadFailure::NotFound {
            thread: ThreadId("gone-thread".into()),
        }),
        "Thread gone-thread doesn't exist on this Forge.".into(),
    );
    let unavailable = opened(serde_json::to_value(&refusal).expect("encode"));
    assert!(
        matches!(unavailable, editor_core::ThreadUnavailable::Missing),
        "{unavailable:?}"
    );
    assert_eq!(
        unavailable.problem().headline(),
        "This thread isn't on this Forge."
    );
}

#[test]
fn an_older_forge_refusing_a_thread_shows_the_generic_failure() {
    let unavailable = opened(serde_json::json!({
        "code": "not_found",
        "message": "thread gone-thread not found",
    }));
    let editor_core::ThreadUnavailable::Failed(error) = &unavailable else {
        panic!("a legacy refusal is not read as a typed missing thread: {unavailable:?}");
    };
    assert_eq!(
        error.source.refused(),
        Some(&artisan_api::Failure::Legacy {
            code: artisan_api::ErrorCode::NotFound
        })
    );
    assert_eq!(
        unavailable.problem().headline(),
        "Couldn't load this thread. Forge didn't recognise it."
    );
}
