use artisan_api::{
    Event, Thread, ThreadId, ThreadsPage,
    tls::{Identity, fingerprint},
};
use editor_core::runtime::runtime;
use editor_core::{
    Core, EditorFiles, ForgeClient, HostConfig, HostId, HostKind, ThreadPageDirection,
};
use std::net::TcpListener;
use std::time::Duration;
use test_support::wait::wait_until;

struct Silent {
    _directory: tempfile::TempDir,
    _listener: TcpListener,
    core: Core,
    host: HostId,
}

fn silent_host() -> Silent {
    let directory = tempfile::tempdir().expect("temporary directory");
    let mut core = Core::for_tests(EditorFiles {
        dir: directory.path().to_owned(),
    })
    .expect("core opens");
    let listener = TcpListener::bind("127.0.0.1:0").expect("a Forge that never answers");
    let identity = Identity::generate("pages-test").expect("identity");
    let pin = fingerprint(identity.certificate_der().expect("certificate").as_ref());
    let config = HostConfig {
        id: HostId("silent-forge".into()),
        name: "Silent".into(),
        address: listener.local_addr().expect("address").to_string(),
        fingerprint: pin.clone(),
        kind: HostKind::Remote,
    };
    let client = ForgeClient::new(&config.address, &pin, &identity).expect("client");
    core.add_test_host(&config, client);
    core.host_mut(&config.id)
        .expect("host")
        .test_stop_connection();
    Silent {
        _directory: directory,
        _listener: listener,
        core,
        host: config.id,
    }
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

impl Silent {
    fn ids(&self, archived: bool) -> Vec<String> {
        let host = self.core.host(&self.host).expect("host");
        host.thread_page(archived)
            .ids()
            .iter()
            .map(|id| id.0.clone())
            .collect()
    }

    fn title(&self, id: &str) -> Option<String> {
        let host = self.core.host(&self.host).expect("host");
        host.threads
            .get(&ThreadId(id.into()))
            .and_then(|thread| thread.title.clone())
    }

    fn start_loading(&mut self, archived: bool, direction: ThreadPageDirection) -> u64 {
        let host = self.core.host_mut(&self.host).expect("host");
        host.load_threads(archived, direction);
        assert!(
            host.thread_page(archived).is_loading(),
            "the page starts loading"
        );
        host.test_thread_page_generation(archived)
    }

    fn generation(&self, archived: bool) -> u64 {
        let host = self.core.host(&self.host).expect("host");
        host.test_thread_page_generation(archived)
    }

    fn settle(&mut self, archived: bool) {
        let host = self.host.clone();
        wait_until(
            "the page completion applies",
            Duration::from_secs(10),
            || {
                self.core.drain_with(|_, _| {});
                !self
                    .core
                    .host(&host)
                    .expect("host")
                    .thread_page(archived)
                    .is_loading()
            },
        );
    }
}

#[test]
fn a_page_that_started_before_newer_events_does_not_roll_them_back() {
    let mut forge = silent_host();
    let generation = forge.start_loading(false, ThreadPageDirection::Latest);
    let mut renamed = thread("renamed", 9, 9, false);
    renamed.title = Some("renamed later".into());
    let sender = forge
        .core
        .host(&forge.host)
        .expect("host")
        .test_transition_sender();
    runtime().block_on(async {
        sender.event(Event::Thread { thread: renamed }).await;
        sender
            .event(Event::ThreadRemoved {
                thread_id: ThreadId("deleted".into()),
            })
            .await;
        sender
            .thread_page(
                false,
                ThreadPageDirection::Latest,
                0,
                generation,
                page(vec![
                    thread("renamed", 3, 3, false),
                    thread("deleted", 2, 2, false),
                    thread("kept", 1, 1, false),
                ]),
            )
            .await;
    });
    forge.settle(false);
    assert_eq!(
        forge.ids(false),
        ["renamed", "kept"],
        "a deleted thread stays deleted"
    );
    assert_eq!(
        forge.title("renamed").as_deref(),
        Some("renamed later"),
        "the page's older copy does not replace the newer event"
    );
    assert_eq!(forge.title("deleted"), None);
}

#[test]
fn a_superseded_page_is_ignored_and_the_current_one_applies() {
    let mut forge = silent_host();
    let current = forge.start_loading(false, ThreadPageDirection::Latest);
    let sender = forge
        .core
        .host(&forge.host)
        .expect("host")
        .test_transition_sender();
    runtime().block_on(async {
        sender
            .thread_page(
                false,
                ThreadPageDirection::Latest,
                0,
                current.wrapping_sub(1),
                page(vec![thread("stale", 1, 1, false)]),
            )
            .await;
        sender
            .thread_page(
                false,
                ThreadPageDirection::Latest,
                0,
                current,
                page(vec![thread("current", 1, 1, false)]),
            )
            .await;
    });
    forge.settle(false);
    assert_eq!(forge.ids(false), ["current"]);
    assert_eq!(
        forge.title("stale"),
        None,
        "nothing from the superseded page is kept"
    );
}

#[test]
fn archived_threads_land_only_in_the_archive_and_older_pages_append() {
    let mut forge = silent_host();
    let first = forge.start_loading(false, ThreadPageDirection::Latest);
    let sender = forge
        .core
        .host(&forge.host)
        .expect("host")
        .test_transition_sender();
    runtime().block_on(async {
        sender
            .thread_page(
                false,
                ThreadPageDirection::Latest,
                0,
                first,
                ThreadsPage {
                    next: Some(artisan_api::ThreadCursor {
                        activity_seq: 5,
                        id: ThreadId("newest".into()),
                    }),
                    ..page(vec![
                        thread("newest", 6, 6, false),
                        thread("archived", 5, 5, true),
                    ])
                },
            )
            .await;
    });
    forge.settle(false);
    assert_eq!(
        forge.ids(false),
        ["newest"],
        "archived threads stay out of the inbox"
    );
    assert!(
        forge
            .core
            .host(&forge.host)
            .expect("host")
            .thread_page(false)
            .has_older(),
        "older threads wait until the list scrolls to them"
    );
    forge.start_loading(false, ThreadPageDirection::Older);
    let older = forge.generation(false);
    let sender = forge
        .core
        .host(&forge.host)
        .expect("host")
        .test_transition_sender();
    runtime().block_on(async {
        sender
            .thread_page(
                false,
                ThreadPageDirection::Older,
                6,
                older,
                page(vec![thread("oldest", 1, 1, false)]),
            )
            .await;
    });
    forge.settle(false);
    assert_eq!(
        forge.ids(false),
        ["newest", "oldest"],
        "older pages add below"
    );
    assert!(
        forge.ids(true).is_empty(),
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

impl Silent {
    fn send(&self, deliver: impl AsyncFnOnce(&editor_core::host::TestHostTransitions)) {
        let sender = self
            .core
            .host(&self.host)
            .expect("host")
            .test_transition_sender();
        runtime().block_on(deliver(&sender));
    }

    fn drain_until(&mut self, what: &str, mut done: impl FnMut(&editor_core::HostData) -> bool) {
        let host = self.host.clone();
        wait_until(what, Duration::from_secs(10), || {
            self.core.drain_with(|_, _| {});
            done(self.core.host(&host).expect("host"))
        });
    }

    fn working(&self, thread: &str) -> bool {
        self.core
            .host(&self.host)
            .expect("host")
            .is_working(&ThreadId(thread.into()))
    }
}

#[test]
fn a_page_loaded_during_a_stop_keeps_the_run_stopped() {
    let mut forge = silent_host();
    forge.send(async |sender| {
        sender
            .event(Event::Thread {
                thread: thread("busy", 1, 1, false),
            })
            .await;
        sender
            .event(Event::Run {
                run: run("busy", 2, artisan_api::RunState::Running),
            })
            .await;
    });
    forge.drain_until("the run arrives", |host| {
        host.runs.contains_key(&ThreadId("busy".into()))
    });
    forge
        .core
        .host_mut(&forge.host)
        .expect("host")
        .stop_optimistically(&ThreadId("busy".into()));
    assert!(!forge.working("busy"), "Stop shows the run stopped at once");
    let generation = forge.start_loading(false, ThreadPageDirection::Latest);
    forge.send(async |sender| {
        sender
            .thread_page(
                false,
                ThreadPageDirection::Latest,
                0,
                generation,
                ThreadsPage {
                    runs: vec![run("busy", 2, artisan_api::RunState::Running)],
                    ..page(vec![thread("busy", 1, 1, false)])
                },
            )
            .await;
    });
    forge.settle(false);
    assert!(
        !forge.working("busy"),
        "a page that left Forge before the stop does not bring the run back"
    );
}

#[test]
fn a_page_loaded_before_a_delete_does_not_bring_back_its_running_run() {
    let mut forge = silent_host();
    let generation = forge.start_loading(false, ThreadPageDirection::Latest);
    forge.send(async |sender| {
        sender
            .event(Event::ThreadRemoved {
                thread_id: ThreadId("deleted".into()),
            })
            .await;
        sender
            .thread_page(
                false,
                ThreadPageDirection::Latest,
                0,
                generation,
                ThreadsPage {
                    runs: vec![run("deleted", 2, artisan_api::RunState::Running)],
                    ..page(vec![thread("deleted", 1, 1, false)])
                },
            )
            .await;
    });
    forge.settle(false);
    assert!(!forge.working("deleted"));
    assert!(
        !forge
            .core
            .host(&forge.host)
            .expect("host")
            .runs
            .contains_key(&ThreadId("deleted".into())),
        "the deleted thread's run stays forgotten"
    );
}

#[test]
fn a_queued_older_run_event_does_not_undo_a_newer_page() {
    let mut forge = silent_host();
    let generation = forge.start_loading(false, ThreadPageDirection::Latest);
    forge.send(async |sender| {
        sender
            .thread_page(
                false,
                ThreadPageDirection::Latest,
                0,
                generation,
                ThreadsPage {
                    runs: vec![run("finished", 9, done())],
                    ..page(vec![thread("finished", 1, 1, false)])
                },
            )
            .await;
        sender
            .event(Event::Run {
                run: run("finished", 5, artisan_api::RunState::Running),
            })
            .await;
    });
    forge.settle(false);
    forge.drain_until("the queued event is applied", |_| true);
    assert!(
        !forge.working("finished"),
        "the finished run stays finished"
    );
}

#[test]
fn scrolling_far_back_keeps_a_bounded_window_and_reloads_newer_threads() {
    let mut forge = silent_host();
    let sender = forge
        .core
        .host(&forge.host)
        .expect("host")
        .test_transition_sender();
    let mut activity = 10_000;
    let mut direction = ThreadPageDirection::Latest;
    for _ in 0..8 {
        let generation = forge.start_loading(false, direction);
        let threads: Vec<Thread> = (0..200)
            .map(|_| {
                activity -= 1;
                let mut read = thread(&format!("t{activity}"), activity, activity, false);
                read.read_seq = activity;
                read
            })
            .collect();
        let last = threads.last().expect("a full page").id.clone();
        runtime().block_on(sender.thread_page(
            false,
            direction,
            0,
            generation,
            ThreadsPage {
                next: Some(artisan_api::ThreadCursor {
                    activity_seq: activity,
                    id: last,
                }),
                ..page(threads)
            },
        ));
        forge.settle(false);
        direction = ThreadPageDirection::Older;
    }
    let ids = forge.ids(false);
    assert_eq!(ids.len(), 1_000, "the list keeps five pages");
    assert_eq!(ids.first().map(String::as_str), Some("t9399"));
    assert_eq!(ids.last().map(String::as_str), Some("t8400"));
    let host = forge.core.host(&forge.host).expect("host");
    assert!(
        !host.thread_page(false).at_latest(),
        "scrolling back up asks for the newer threads again"
    );
    assert!(
        host.threads.len() <= 1_000,
        "threads outside the window are released"
    );
}

fn refusing_forge(
    refusal: serde_json::Value,
) -> (
    tempfile::TempDir,
    test_support::http::StubServer,
    Core,
    HostId,
) {
    let router = axum::Router::new().route(
        "/threads/{id}",
        axum::routing::get(move || {
            let refusal = refusal.clone();
            async move { (axum::http::StatusCode::NOT_FOUND, axum::Json(refusal)) }
        }),
    );
    let server = runtime().block_on(test_support::http::StubServer::serve(router));
    let directory = tempfile::tempdir().expect("temporary directory");
    let mut core = Core::for_tests(EditorFiles {
        dir: directory.path().to_owned(),
    })
    .expect("core opens");
    let identity = Identity::generate("refusing-forge").expect("identity");
    let config = HostConfig {
        id: HostId("refusing-forge".into()),
        name: "Refusing".into(),
        address: server.url().trim_start_matches("http://").to_owned(),
        fingerprint: fingerprint(identity.certificate_der().expect("certificate").as_ref()),
        kind: HostKind::Remote,
    };
    core.add_test_host(&config, ForgeClient::plain_for_tests(server.url()));
    core.host_mut(&config.id)
        .expect("host")
        .test_stop_connection();
    (directory, server, core, config.id)
}

fn opened(refusal: serde_json::Value) -> editor_core::ThreadUnavailable {
    let (_directory, _server, mut core, host) = refusing_forge(refusal);
    let thread = ThreadId("gone-thread".into());
    core.navigate(editor_core::Screen::Thread {
        host: host.clone(),
        thread: thread.clone(),
    });
    let mut unavailable = None;
    wait_until("the thread lookup settles", Duration::from_secs(20), || {
        core.drain_with(|_, _| {});
        unavailable = core
            .host(&host)
            .and_then(|forge| forge.thread_unavailable(&thread))
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
        "{unavailable}"
    );
    assert_eq!(unavailable.to_string(), "This thread isn't on this Forge.");
}

#[test]
fn an_older_forge_refusing_a_thread_shows_the_generic_failure() {
    let unavailable = opened(serde_json::json!({
        "code": "not_found",
        "message": "thread gone-thread not found",
    }));
    let editor_core::ThreadUnavailable::Failed(error) = &unavailable else {
        panic!("a legacy refusal is not read as a typed missing thread: {unavailable}");
    };
    assert_eq!(
        error.source.refused(),
        Some(&artisan_api::Failure::Legacy {
            code: artisan_api::ErrorCode::NotFound
        })
    );
    assert_eq!(
        unavailable.to_string(),
        "Couldn't load this thread. Forge didn't recognise it."
    );
}
