use artisan_api::{
    EngineConfig, EngineId, Event, Permissions, ProjectId, Thread, ThreadCursor, ThreadId,
    ThreadsPage,
};
use axum::http::StatusCode;
use editor_core::{EditReport, LocalSend, NewThread, ThreadEdit, ThreadPageDirection};
use std::sync::mpsc;
use test_support::editor::core::StubbedHost;
use test_support::editor::forge::empty_state;

fn thread(id: &str, seq: u64, activity: u64) -> Thread {
    serde_json::from_value(serde_json::json!({
        "id": id,
        "project_id": "project",
        "title": id,
        "config": {"engine": "claude"},
        "activity_seq": activity,
        "read_seq": activity,
        "seq": seq,
        "created_at": 0,
        "updated_at": 0,
        "archived": false,
    }))
    .expect("thread")
}

fn titled(id: &str, seq: u64, title: &str) -> Thread {
    Thread {
        title: Some(title.to_owned()),
        ..thread(id, seq, 1)
    }
}

fn shown(forge: &StubbedHost, id: &str) -> Option<Thread> {
    forge.host().threads.get(&ThreadId(id.into())).cloned()
}

fn title(forge: &StubbedHost, id: &str) -> Option<String> {
    shown(forge, id).and_then(|thread| thread.title)
}

fn inbox(forge: &StubbedHost) -> Vec<String> {
    forge
        .host()
        .thread_page(false)
        .ids()
        .iter()
        .map(|id| id.0.clone())
        .collect()
}

fn publish_and_apply(forge: &mut StubbedHost, events: Vec<Event>) {
    for event in events {
        forge.forge.publish(event);
    }
    forge.apply_published();
}

fn reports() -> (EditReport, mpsc::Receiver<Result<(), String>>) {
    let (sender, receiver) = mpsc::channel();
    let report: EditReport = Box::new(move |result| {
        sender
            .send(result.map_err(|error| error.to_string()))
            .expect("the test keeps the report receiver");
        Box::pin(async {})
    });
    (report, receiver)
}

fn edit(forge: &mut StubbedHost, id: &str, edit: ThreadEdit) -> mpsc::Receiver<Result<(), String>> {
    let (report, receiver) = reports();
    forge
        .host_mut()
        .edit_thread(&ThreadId(id.into()), edit, report);
    receiver
}

fn reported(
    forge: &mut StubbedHost,
    receiver: &mpsc::Receiver<Result<(), String>>,
) -> Result<(), String> {
    let mut outcome = None;
    forge.settle("the edit reports its outcome", |_, _| {
        if outcome.is_none() {
            outcome = match receiver.try_recv() {
                Ok(reported) => Some(reported),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    panic!("the edit dropped its report without sending it")
                }
            };
        }
        outcome.is_some()
    });
    outcome.expect("invariant: settle returned once an outcome arrived")
}

#[test]
fn a_rename_shows_at_once_and_older_echoes_never_revert_it() {
    let mut forge = StubbedHost::start(empty_state(), axum::Router::new());
    publish_and_apply(
        &mut forge,
        vec![Event::Thread {
            thread: titled("chat", 3, "before"),
        }],
    );
    let outcome = edit(&mut forge, "chat", ThreadEdit::Rename("after".into()));
    assert_eq!(
        title(&forge, "chat").as_deref(),
        Some("after"),
        "the new title shows before Forge answers"
    );
    let rename = forge.take("PATCH", "/threads/chat");
    publish_and_apply(
        &mut forge,
        vec![Event::Thread {
            thread: titled("chat", 4, "before"),
        }],
    );
    assert_eq!(
        title(&forge, "chat").as_deref(),
        Some("after"),
        "an echo of an earlier change does not undo the rename in flight"
    );
    rename.respond(titled("chat", 6, "after"));
    assert_eq!(reported(&mut forge, &outcome), Ok(()));
    publish_and_apply(
        &mut forge,
        vec![Event::Thread {
            thread: titled("chat", 5, "before"),
        }],
    );
    assert_eq!(
        title(&forge, "chat").as_deref(),
        Some("after"),
        "an echo older than Forge's reply does not undo the rename"
    );
}

#[test]
fn a_failed_archive_puts_the_thread_back_and_reports_why() {
    let mut forge = StubbedHost::start(empty_state(), axum::Router::new());
    publish_and_apply(
        &mut forge,
        vec![Event::Thread {
            thread: thread("chat", 3, 3),
        }],
    );
    assert_eq!(inbox(&forge), ["chat"]);
    let outcome = edit(&mut forge, "chat", ThreadEdit::Archive);
    assert!(
        inbox(&forge).is_empty(),
        "an archived thread leaves the list before Forge answers"
    );
    assert!(shown(&forge, "chat").is_some_and(|thread| thread.archived));
    forge.take("PATCH", "/threads/chat").fail(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal",
        "disk full",
    );
    let error = reported(&mut forge, &outcome).expect_err("the archive failed");
    assert!(
        error.contains("archive for thread chat"),
        "the failure names the action and thread: {error}"
    );
    assert_eq!(inbox(&forge), ["chat"], "the thread comes back to the list");
    assert!(shown(&forge, "chat").is_some_and(|thread| !thread.archived));
}

#[test]
fn a_later_edit_survives_an_earlier_one_failing() {
    let mut forge = StubbedHost::start(empty_state(), axum::Router::new());
    publish_and_apply(
        &mut forge,
        vec![Event::Thread {
            thread: thread("chat", 3, 3),
        }],
    );
    let unread = edit(&mut forge, "chat", ThreadEdit::MarkUnread);
    let mark = forge.take("POST", "/threads/chat/unread");
    let renamed = edit(&mut forge, "chat", ThreadEdit::Rename("kept".into()));
    let rename = forge.take("PATCH", "/threads/chat");
    mark.fail(StatusCode::INTERNAL_SERVER_ERROR, "internal", "disk full");
    assert!(reported(&mut forge, &unread).is_err());
    let after = shown(&forge, "chat").expect("the thread stays");
    assert!(!after.marked_unread, "the failed mark is rolled back");
    assert_eq!(
        after.title.as_deref(),
        Some("kept"),
        "the rename still in flight keeps showing"
    );
    rename.respond(titled("chat", 5, "kept"));
    assert_eq!(reported(&mut forge, &renamed), Ok(()));
    assert_eq!(title(&forge, "chat").as_deref(), Some("kept"));
}

fn older_page(from: u64) -> ThreadsPage {
    let threads: Vec<Thread> = (0..200)
        .map(|offset| {
            let activity = from - offset;
            thread(&format!("old-{activity:05}"), activity, activity)
        })
        .collect();
    let last = threads.last().expect("a page has threads");
    ThreadsPage {
        next: Some(ThreadCursor {
            activity_seq: last.activity_seq,
            id: last.id.clone(),
        }),
        threads,
        runs: Vec::new(),
        newer: false,
    }
}

#[test]
fn a_new_thread_stays_listed_while_history_is_paged_past_the_newest_threads() {
    let mut state = empty_state();
    state.threads_next = Some(ThreadCursor {
        activity_seq: 10_000,
        id: ThreadId("cursor".into()),
    });
    let mut forge = StubbedHost::start(state, axum::Router::new());
    for page in 0..6 {
        forge
            .host_mut()
            .load_threads(false, ThreadPageDirection::Older);
        forge
            .take("GET", "/threads")
            .respond(older_page(10_000 - page * 200));
        forge.settle("the older page applies", |core, host| {
            core.host(host)
                .is_some_and(|data| !data.thread_page(false).is_loading())
        });
    }
    assert!(
        !forge.host().thread_page(false).at_latest(),
        "the history is paged past its newest threads"
    );
    let sent = new_thread(&forge, "new");
    forge.host_mut().begin_send(sent);
    publish_and_apply(
        &mut forge,
        vec![
            Event::Thread {
                thread: thread("new", 20_000, 20_000),
            },
            Event::Thread {
                thread: thread("other", 20_001, 20_001),
            },
        ],
    );
    assert!(
        shown(&forge, "new").is_some(),
        "the thread just sent from this Editor stays in the sidebar"
    );
    assert!(
        shown(&forge, "other").is_none(),
        "a read thread nobody is waiting for is still pruned"
    );
    forge
        .host_mut()
        .load_threads(false, ThreadPageDirection::Older);
    forge.take("GET", "/threads").respond(older_page(8_800));
    forge.settle("the older page applies", |core, host| {
        core.host(host)
            .is_some_and(|data| !data.thread_page(false).is_loading())
    });
    assert!(
        shown(&forge, "new").is_some(),
        "the new thread survives the prune after the next page"
    );
}

fn hand_back(forge: &mut StubbedHost, sent: &LocalSend) {
    forge.host_mut().settle_send(
        sent.attempt,
        editor_core::SendSettlement::HandedBack(editor_core::HandBack::Closed),
    );
}

fn new_thread(forge: &StubbedHost, id: &str) -> LocalSend {
    LocalSend {
        attempt: forge.host().send_receipt().attempt(),
        thread: ThreadId(id.into()),
        message: artisan_api::ItemId(format!("{id}-message")),
        text: "hello".into(),
        images: Vec::new(),
        create: Some(NewThread {
            project: ProjectId("project".into()),
            config: EngineConfig::new(EngineId::Claude),
        }),
    }
}

fn page_older(forge: &mut StubbedHost, from: u64) {
    forge
        .host_mut()
        .load_threads(false, ThreadPageDirection::Older);
    forge.take("GET", "/threads").respond(older_page(from));
    forge.settle("the older page applies", |core, host| {
        core.host(host)
            .is_some_and(|data| !data.thread_page(false).is_loading())
    });
}

fn paged_history() -> StubbedHost {
    let mut state = empty_state();
    state.threads_next = Some(ThreadCursor {
        activity_seq: 10_000,
        id: ThreadId("cursor".into()),
    });
    StubbedHost::start(state, axum::Router::new())
}

fn no_request(forge: &mut StubbedHost, method: &str, path: &str) {
    forge.apply_published();
    assert!(
        forge.forge.try_take(method, path).is_none(),
        "the Editor sent {method} {path} before the earlier edit was answered"
    );
}

#[test]
fn renames_go_out_one_at_a_time_and_the_last_one_wins_whatever_order_forge_answers() {
    let mut forge = StubbedHost::start(empty_state(), axum::Router::new());
    publish_and_apply(
        &mut forge,
        vec![Event::Thread {
            thread: titled("chat", 3, "before"),
        }],
    );
    let first = edit(&mut forge, "chat", ThreadEdit::Rename("first".into()));
    let first_request = forge.take("PATCH", "/threads/chat");
    let second = edit(&mut forge, "chat", ThreadEdit::Rename("second".into()));
    assert_eq!(title(&forge, "chat").as_deref(), Some("second"));
    no_request(&mut forge, "PATCH", "/threads/chat");
    first_request.respond(titled("chat", 4, "first"));
    assert_eq!(reported(&mut forge, &first), Ok(()));
    assert_eq!(
        title(&forge, "chat").as_deref(),
        Some("second"),
        "the earlier rename's answer does not show over the later one"
    );
    forge
        .take("PATCH", "/threads/chat")
        .respond(titled("chat", 5, "second"));
    assert_eq!(reported(&mut forge, &second), Ok(()));
    assert_eq!(title(&forge, "chat").as_deref(), Some("second"));
}

#[test]
fn a_report_that_takes_time_still_arrives_after_the_edit_settles() {
    let mut forge = StubbedHost::start(empty_state(), axum::Router::new());
    publish_and_apply(
        &mut forge,
        vec![Event::Thread {
            thread: thread("chat", 3, 3),
        }],
    );
    let (sender, receiver) = mpsc::channel();
    let (open_gate, gate) = tokio::sync::oneshot::channel::<()>();
    let report: EditReport = Box::new(move |result| {
        Box::pin(async move {
            gate.await.expect("the test opens the gate");
            sender
                .send(result.map_err(|error| error.to_string()))
                .expect("the test keeps the report receiver");
        })
    });
    forge
        .host_mut()
        .edit_thread(&ThreadId("chat".into()), ThreadEdit::Archive, report);
    forge.take("PATCH", "/threads/chat").fail(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal",
        "disk full",
    );
    forge.settle("the failed archive is applied", |core, host| {
        core.host(host)
            .is_some_and(|data| !data.has_requests_in_flight())
    });
    open_gate
        .send(())
        .expect("the report is still waiting at the gate");
    assert!(
        reported(&mut forge, &receiver).is_err(),
        "the failure reaches the caller even though applying it ended the edit"
    );
}

#[test]
fn an_edit_on_a_thread_the_editor_does_not_have_reports_why() {
    let mut forge = StubbedHost::start(empty_state(), axum::Router::new());
    let outcome = edit(&mut forge, "missing", ThreadEdit::Rename("x".into()));
    let error = reported(&mut forge, &outcome).expect_err("nothing to rename");
    assert!(
        error.contains("doesn't have the thread"),
        "the caller learns the thread is unknown: {error}"
    );
}

#[test]
fn a_confirmed_archive_is_not_undone_by_a_late_echo_after_its_row_is_pruned() {
    let mut forge = StubbedHost::start(empty_state(), axum::Router::new());
    publish_and_apply(
        &mut forge,
        vec![Event::Thread {
            thread: thread("chat", 3, 3),
        }],
    );
    let outcome = edit(&mut forge, "chat", ThreadEdit::Archive);
    let mut archived = thread("chat", 6, 3);
    archived.archived = true;
    forge.take("PATCH", "/threads/chat").respond(archived);
    assert_eq!(reported(&mut forge, &outcome), Ok(()));
    publish_and_apply(
        &mut forge,
        vec![Event::Thread {
            thread: thread("other", 7, 7),
        }],
    );
    assert!(
        shown(&forge, "chat").is_none(),
        "the archived row leaves the inbox and is pruned"
    );
    publish_and_apply(
        &mut forge,
        vec![Event::Thread {
            thread: thread("chat", 4, 3),
        }],
    );
    assert!(
        !inbox(&forge).contains(&"chat".to_owned()),
        "an echo from before the archive does not bring the row back"
    );
}

#[test]
fn a_new_thread_stays_through_older_pages_until_forge_lists_it() {
    let mut forge = paged_history();
    let sent = new_thread(&forge, "new");
    forge.host_mut().begin_send(sent);
    publish_and_apply(
        &mut forge,
        vec![Event::Thread {
            thread: thread("new", 20_000, 20_000),
        }],
    );
    assert_eq!(inbox(&forge), ["new"], "the echo joins the newest page");
    for page in 0..6 {
        page_older(&mut forge, 10_000 - page * 200);
    }
    assert!(
        !inbox(&forge).contains(&"new".to_owned()),
        "paging back trims the newest rows from the window"
    );
    assert!(
        shown(&forge, "new").is_some(),
        "the new thread stays because no Forge list has included it yet"
    );
}

#[test]
fn a_failed_archive_returns_the_row_to_the_older_window_it_was_in() {
    let mut forge = paged_history();
    for page in 0..6 {
        page_older(&mut forge, 10_000 - page * 200);
    }
    assert!(!forge.host().thread_page(false).at_latest());
    let row = "old-09000";
    assert!(inbox(&forge).contains(&row.to_owned()));
    let outcome = edit(&mut forge, row, ThreadEdit::Archive);
    assert!(!inbox(&forge).contains(&row.to_owned()));
    forge.take("PATCH", &format!("/threads/{row}")).fail(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal",
        "disk full",
    );
    assert!(reported(&mut forge, &outcome).is_err());
    assert!(
        inbox(&forge).contains(&row.to_owned()),
        "the row comes back to the window it was archived from"
    );
}

#[test]
fn a_permission_change_in_flight_leaves_the_confirmed_config_alone() {
    let mut forge = StubbedHost::start(empty_state(), axum::Router::new());
    publish_and_apply(
        &mut forge,
        vec![Event::Thread {
            thread: thread("chat", 3, 3),
        }],
    );
    let _outcome = edit(
        &mut forge,
        "chat",
        ThreadEdit::Permissions(Permissions::Full),
    );
    let _held = forge.take("PATCH", "/threads/chat");
    let id = ThreadId("chat".into());
    assert_eq!(
        forge.host().threads[&id].config.permissions,
        Permissions::Full,
        "the picker shows the new mode at once"
    );
    assert_eq!(
        forge
            .host()
            .confirmed_thread(&id)
            .map(|thread| thread.config.permissions),
        Some(Permissions::Auto),
        "a message sent now compares against what Forge has, so it carries the new mode"
    );
}

#[test]
fn read_unread_and_read_again_go_out_in_order_and_the_last_read_wins() {
    let mut forge = StubbedHost::start(empty_state(), axum::Router::new());
    let id = ThreadId("chat".into());
    let mut unread = thread("chat", 3, 5);
    unread.read_seq = 0;
    forge.host_mut().observe_thread(Some(id.clone()));
    publish_and_apply(&mut forge, vec![Event::Thread { thread: unread }]);
    forge.host_mut().acknowledge_read(&id, 5);
    let first_read = forge.take("PATCH", "/threads/chat");
    let marked = edit(&mut forge, "chat", ThreadEdit::MarkUnread);
    forge.host_mut().observe_thread(None);
    forge.host_mut().observe_thread(Some(id.clone()));
    forge.host_mut().acknowledge_read(&id, 5);
    no_request(&mut forge, "POST", "/threads/chat/unread");
    let mut read = thread("chat", 4, 5);
    read.read_seq = 5;
    first_read.respond(read.clone());
    let mut marked_reply = read.clone();
    marked_reply.seq = 5;
    marked_reply.marked_unread = true;
    forge
        .take("POST", "/threads/chat/unread")
        .respond(marked_reply);
    assert_eq!(reported(&mut forge, &marked), Ok(()));
    let mut reread = read;
    reread.seq = 6;
    forge.take("PATCH", "/threads/chat").respond(reread);
    forge.settle("the last read is confirmed", |core, host| {
        core.host(host)
            .and_then(|data| data.threads.get(&ThreadId("chat".into())))
            .is_some_and(|thread| thread.seq == 6)
    });
    assert!(
        !forge.host().threads[&id].is_unread(),
        "reading again after marking unread leaves the thread read"
    );
}

fn not_found(id: &str) -> serde_json::Value {
    serde_json::to_value(artisan_api::ApiError::new(
        artisan_api::Failure::Thread(artisan_api::ThreadFailure::NotFound {
            thread: ThreadId(id.into()),
        }),
        format!("Thread {id} doesn't exist on this Forge."),
    ))
    .expect("encode")
}

#[test]
fn a_thread_created_before_a_reconnect_is_ready_once_the_snapshot_lists_it() {
    let mut forge = StubbedHost::start(empty_state(), axum::Router::new());
    let sent = new_thread(&forge, "new");
    forge.host_mut().begin_send(sent);
    assert!(forge.host().is_provisional(&ThreadId("new".into())));
    forge.forge.list_thread(thread("new", 4, 4));
    forge.forge.publish(Event::Reset);
    forge.settle("the reconnect snapshot applies", |core, host| {
        core.host(host)
            .is_some_and(|data| !data.is_provisional(&ThreadId("new".into())))
    });
    let renamed = edit(&mut forge, "new", ThreadEdit::Rename("named".into()));
    forge
        .take("PATCH", "/threads/new")
        .respond(titled("new", 5, "named"));
    assert_eq!(
        reported(&mut forge, &renamed),
        Ok(()),
        "the thread can be edited without waiting for another event"
    );
}

#[test]
fn a_late_reply_for_another_field_does_not_lower_the_confirmed_seq() {
    let mut forge = StubbedHost::start(empty_state(), axum::Router::new());
    publish_and_apply(
        &mut forge,
        vec![Event::Thread {
            thread: thread("chat", 3, 3),
        }],
    );
    let archived = edit(&mut forge, "chat", ThreadEdit::Archive);
    let archive = forge.take("PATCH", "/threads/chat");
    let renamed = edit(&mut forge, "chat", ThreadEdit::Rename("named".into()));
    let rename = forge.take("PATCH", "/threads/chat");
    let mut archived_reply = thread("chat", 6, 3);
    archived_reply.archived = true;
    archive.respond(archived_reply);
    assert_eq!(reported(&mut forge, &archived), Ok(()));
    rename.respond(titled("chat", 4, "named"));
    assert_eq!(reported(&mut forge, &renamed), Ok(()));
    publish_and_apply(
        &mut forge,
        vec![Event::Thread {
            thread: thread("other", 7, 7),
        }],
    );
    publish_and_apply(
        &mut forge,
        vec![Event::Thread {
            thread: thread("chat", 5, 3),
        }],
    );
    assert!(
        !inbox(&forge).contains(&"chat".to_owned()),
        "an echo from before the archive does not bring the row back"
    );
}

#[test]
fn a_rename_answered_after_paging_away_leaves_the_window_where_the_user_paged() {
    let mut forge = paged_history();
    for page in 0..6 {
        page_older(&mut forge, 10_000 - page * 200);
    }
    let row = "old-09000";
    assert!(inbox(&forge).contains(&row.to_owned()));
    let renamed = edit(&mut forge, row, ThreadEdit::Rename("named".into()));
    let rename = forge.take("PATCH", &format!("/threads/{row}"));
    for page in 0..5 {
        page_older(&mut forge, 8_800 - page * 200);
    }
    assert!(
        !inbox(&forge).contains(&row.to_owned()),
        "paging further back moves the row out of the window"
    );
    let mut reply = thread(row, 9_001, 9_000);
    reply.title = Some("named".into());
    rename.respond(reply);
    assert_eq!(reported(&mut forge, &renamed), Ok(()));
    assert!(
        !inbox(&forge).contains(&row.to_owned()),
        "the answered rename does not pull the row back into the window"
    );
}

#[test]
fn edits_on_a_thread_removed_meanwhile_report_that_it_was_removed() {
    let mut forge = StubbedHost::start(empty_state(), axum::Router::new());
    publish_and_apply(
        &mut forge,
        vec![Event::Thread {
            thread: thread("chat", 3, 3),
        }],
    );
    let archived = edit(&mut forge, "chat", ThreadEdit::Archive);
    let _archive = forge.take("PATCH", "/threads/chat");
    let renamed = edit(&mut forge, "chat", ThreadEdit::Rename("named".into()));
    let _rename = forge.take("PATCH", "/threads/chat");
    publish_and_apply(
        &mut forge,
        vec![Event::ThreadRemoved {
            thread_id: ThreadId("chat".into()),
        }],
    );
    for outcome in [&archived, &renamed] {
        let error = reported(&mut forge, outcome).expect_err("the thread is gone");
        assert!(
            error.contains("the thread was removed"),
            "the caller learns the thread is gone: {error}"
        );
    }
}

#[test]
fn a_thread_deleted_while_disconnected_does_not_come_back_with_its_edit() {
    let mut forge = StubbedHost::start(empty_state(), axum::Router::new());
    publish_and_apply(
        &mut forge,
        vec![Event::Thread {
            thread: thread("chat", 3, 3),
        }],
    );
    let renamed = edit(&mut forge, "chat", ThreadEdit::Rename("named".into()));
    let _rename = forge.take("PATCH", "/threads/chat");
    forge.forge.publish(Event::Reset);
    let lookup = forge.take("GET", "/threads/chat");
    assert!(
        shown(&forge, "chat").is_none(),
        "the snapshot without the thread does not show it again"
    );
    lookup.respond_with(StatusCode::NOT_FOUND, not_found("chat"));
    let error = reported(&mut forge, &renamed).expect_err("the thread is gone");
    assert!(error.contains("the thread was removed"), "{error}");
    assert!(shown(&forge, "chat").is_none());
}

fn unread(forge: &mut StubbedHost, id: &str) -> ThreadId {
    let mut unread = thread(id, 3, 5);
    unread.read_seq = 0;
    publish_and_apply(forge, vec![Event::Thread { thread: unread }]);
    ThreadId(id.into())
}

fn read(forge: &mut StubbedHost, id: &ThreadId) {
    forge.host_mut().observe_thread(Some(id.clone()));
    forge.host_mut().acknowledge_read(id, 5);
    forge.host_mut().observe_thread(None);
}

#[test]
fn marking_unread_does_not_wait_behind_a_read_that_keeps_failing() {
    let mut forge = StubbedHost::start(empty_state(), axum::Router::new());
    let id = unread(&mut forge, "chat");
    read(&mut forge, &id);
    forge.take("PATCH", "/threads/chat").fail(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal",
        "disk busy",
    );
    let marked = edit(&mut forge, "chat", ThreadEdit::MarkUnread);
    let mut reply = thread("chat", 4, 5);
    reply.read_seq = 0;
    reply.marked_unread = true;
    forge.take("POST", "/threads/chat/unread").respond(reply);
    assert_eq!(reported(&mut forge, &marked), Ok(()));
    assert!(forge.host().threads[&id].is_unread());
    no_request(&mut forge, "PATCH", "/threads/chat");
}

#[test]
fn reads_for_many_threads_go_out_a_few_at_a_time() {
    let mut forge = StubbedHost::start(empty_state(), axum::Router::new());
    let ids: Vec<ThreadId> = (0..6)
        .map(|index| unread(&mut forge, &format!("t{index}")))
        .collect();
    for id in &ids {
        read(&mut forge, id);
    }
    let mut held: Vec<_> = (0..4)
        .map(|index| forge.take("PATCH", &format!("/threads/t{index}")))
        .collect();
    no_request(&mut forge, "PATCH", "/threads/t4");
    let mut first = thread("t0", 4, 5);
    first.read_seq = 5;
    held.remove(0).respond(first);
    let fifth_request = forge.take("PATCH", "/threads/t4");
    no_request(&mut forge, "PATCH", "/threads/t5");
    let mut fifth = thread("t4", 4, 5);
    fifth.read_seq = 5;
    fifth_request.respond(fifth);
    let mut sixth = thread("t5", 4, 5);
    sixth.read_seq = 5;
    forge.take("PATCH", "/threads/t5").respond(sixth);
    forge.settle("the last read is confirmed", |core, host| {
        core.host(host)
            .and_then(|data| data.threads.get(&ThreadId("t5".into())))
            .is_some_and(|thread| !thread.is_unread() && thread.seq == 4)
    });
}

#[test]
fn an_edit_reply_before_the_lookup_does_not_bring_back_a_thread_deleted_while_disconnected() {
    let mut forge = StubbedHost::start(empty_state(), axum::Router::new());
    publish_and_apply(
        &mut forge,
        vec![Event::Thread {
            thread: thread("chat", 3, 3),
        }],
    );
    let renamed = edit(&mut forge, "chat", ThreadEdit::Rename("named".into()));
    let rename = forge.take("PATCH", "/threads/chat");
    forge.forge.publish(Event::Reset);
    let lookup = forge.take("GET", "/threads/chat");
    rename.respond(titled("chat", 5, "named"));
    assert_eq!(reported(&mut forge, &renamed), Ok(()));
    assert!(
        shown(&forge, "chat").is_none(),
        "the reply waits for the lookup instead of showing the row again"
    );
    lookup.respond_with(StatusCode::NOT_FOUND, not_found("chat"));
    forge.settle("the lookup settles", |core, host| {
        core.host(host)
            .is_some_and(|data| data.thread_unavailable(&ThreadId("chat".into())).is_some())
    });
    assert!(shown(&forge, "chat").is_none());
    assert!(!inbox(&forge).contains(&"chat".to_owned()));
}

#[test]
fn an_edit_reply_before_the_lookup_shows_once_the_lookup_finds_the_thread() {
    let mut forge = StubbedHost::start(empty_state(), axum::Router::new());
    publish_and_apply(
        &mut forge,
        vec![Event::Thread {
            thread: thread("chat", 3, 3),
        }],
    );
    let renamed = edit(&mut forge, "chat", ThreadEdit::Rename("named".into()));
    let rename = forge.take("PATCH", "/threads/chat");
    forge.forge.publish(Event::Reset);
    let lookup = forge.take("GET", "/threads/chat");
    rename.respond(titled("chat", 5, "named"));
    assert_eq!(reported(&mut forge, &renamed), Ok(()));
    lookup.respond(titled("chat", 4, "before"));
    forge.settle("the lookup brings the thread back", |core, host| {
        core.host(host)
            .is_some_and(|data| data.threads.contains_key(&ThreadId("chat".into())))
    });
    assert_eq!(
        title(&forge, "chat").as_deref(),
        Some("named"),
        "the confirmed rename wins over the older lookup"
    );
}

#[test]
fn deleting_threads_with_reads_in_flight_lets_a_waiting_read_go_out() {
    let mut forge = StubbedHost::start(empty_state(), axum::Router::new());
    let ids: Vec<ThreadId> = (0..5)
        .map(|index| unread(&mut forge, &format!("t{index}")))
        .collect();
    for id in &ids {
        read(&mut forge, id);
    }
    let _held: Vec<_> = (0..4)
        .map(|index| forge.take("PATCH", &format!("/threads/t{index}")))
        .collect();
    no_request(&mut forge, "PATCH", "/threads/t4");
    publish_and_apply(
        &mut forge,
        (0..4)
            .map(|index| Event::ThreadRemoved {
                thread_id: ThreadId(format!("t{index}")),
            })
            .collect(),
    );
    forge.take("PATCH", "/threads/t4");
}

#[test]
fn a_refused_archive_after_paging_further_back_does_not_join_the_new_window() {
    let mut forge = paged_history();
    for page in 0..6 {
        page_older(&mut forge, 10_000 - page * 200);
    }
    let row = "old-09000";
    let outcome = edit(&mut forge, row, ThreadEdit::Archive);
    let archive = forge.take("PATCH", &format!("/threads/{row}"));
    for page in 0..5 {
        page_older(&mut forge, 8_800 - page * 200);
    }
    archive.fail(StatusCode::INTERNAL_SERVER_ERROR, "internal", "disk full");
    assert!(reported(&mut forge, &outcome).is_err());
    assert!(
        !inbox(&forge).contains(&row.to_owned()),
        "the row does not land in a window it was never part of"
    );
}

#[test]
fn a_first_message_that_fails_after_leaving_its_thread_leaves_no_row_behind() {
    let mut forge = StubbedHost::start(empty_state(), axum::Router::new());
    let id = ThreadId("new".into());
    let sent = new_thread(&forge, "new");
    forge.host_mut().begin_send(sent.clone());
    forge.host_mut().observe_thread(Some(id.clone()));
    forge.host_mut().observe_thread(None);
    hand_back(&mut forge, &sent);
    assert!(shown(&forge, "new").is_none());
    assert!(!forge.host().is_provisional(&id));
}

#[test]
fn an_edit_on_a_thread_still_being_created_goes_out_once_forge_has_it() {
    let mut forge = StubbedHost::start(empty_state(), axum::Router::new());
    let sent = new_thread(&forge, "new");
    forge.host_mut().begin_send(sent);
    let renamed = edit(&mut forge, "new", ThreadEdit::Rename("named".into()));
    assert_eq!(title(&forge, "new").as_deref(), Some("named"));
    no_request(&mut forge, "PATCH", "/threads/new");
    publish_and_apply(
        &mut forge,
        vec![Event::Thread {
            thread: thread("new", 4, 4),
        }],
    );
    forge
        .take("PATCH", "/threads/new")
        .respond(titled("new", 5, "named"));
    assert_eq!(reported(&mut forge, &renamed), Ok(()));
    assert_eq!(title(&forge, "new").as_deref(), Some("named"));
}

#[test]
fn an_edit_on_a_thread_whose_creation_failed_says_so() {
    let mut forge = StubbedHost::start(empty_state(), axum::Router::new());
    let id = ThreadId("new".into());
    let sent = new_thread(&forge, "new");
    forge.host_mut().begin_send(sent.clone());
    forge.host_mut().observe_thread(Some(id));
    let queued = edit(&mut forge, "new", ThreadEdit::Rename("named".into()));
    assert_eq!(title(&forge, "new").as_deref(), Some("named"));
    hand_back(&mut forge, &sent);
    let error = reported(&mut forge, &queued).expect_err("the creation failed");
    assert!(error.contains("creating the thread failed"), "{error}");
    assert_eq!(
        title(&forge, "new"),
        None,
        "the rename that never reached Forge no longer shows on the failed row"
    );
    let later = edit(&mut forge, "new", ThreadEdit::Rename("again".into()));
    let error = reported(&mut forge, &later).expect_err("the creation failed");
    assert!(error.contains("creating the thread failed"), "{error}");
}

#[test]
fn an_edited_retry_under_the_same_thread_shows_the_new_config() {
    let mut forge = StubbedHost::start(empty_state(), axum::Router::new());
    let id = ThreadId("new".into());
    forge.host_mut().observe_thread(Some(id));
    let sent = new_thread(&forge, "new");
    forge.host_mut().begin_send(sent.clone());
    hand_back(&mut forge, &sent);
    let mut retry = new_thread(&forge, "new");
    retry.message = artisan_api::ItemId("retry".into());
    retry.create = Some(NewThread {
        project: ProjectId("project".into()),
        config: EngineConfig::new(EngineId::Codex),
    });
    let replaced = forge.host_mut().begin_send(retry);
    assert!(replaced.is_empty(), "the retry keeps its own row");
    assert_eq!(
        shown(&forge, "new").map(|thread| thread.config.engine),
        Some(EngineId::Codex),
        "the row shows the engine the retry was sent with"
    );
}

#[test]
fn a_reconnect_keeps_a_queued_rename_on_a_thread_still_being_created() {
    let mut forge = StubbedHost::start(empty_state(), axum::Router::new());
    let sent = new_thread(&forge, "new");
    forge.host_mut().begin_send(sent);
    let renamed = edit(&mut forge, "new", ThreadEdit::Rename("named".into()));
    forge.forge.list_thread(thread("marker", 2, 2));
    forge.forge.publish(Event::Reset);
    forge.settle("the snapshot applies", |core, host| {
        core.host(host)
            .is_some_and(|data| data.threads.contains_key(&ThreadId("marker".into())))
    });
    assert_eq!(
        title(&forge, "new").as_deref(),
        Some("named"),
        "the snapshot keeps the queued rename on the provisional row"
    );
    publish_and_apply(
        &mut forge,
        vec![Event::Thread {
            thread: thread("new", 4, 4),
        }],
    );
    forge
        .take("PATCH", "/threads/new")
        .respond(titled("new", 5, "named"));
    assert_eq!(reported(&mut forge, &renamed), Ok(()));
}

fn read_answered(forge: &mut StubbedHost) {
    forge.settle("the read's answer applies", |core, host| {
        core.host(host)
            .is_some_and(|data| !data.has_requests_in_flight())
    });
}

#[test]
fn a_refused_read_is_not_sent_again_while_the_thread_stays_on_screen() {
    let mut forge = StubbedHost::start(empty_state(), axum::Router::new());
    let id = unread(&mut forge, "chat");
    forge.host_mut().observe_thread(Some(id.clone()));
    forge.host_mut().acknowledge_read(&id, 5);
    forge
        .take("PATCH", "/threads/chat")
        .fail(StatusCode::BAD_REQUEST, "bad_request", "not now");
    read_answered(&mut forge);
    assert_eq!(
        forge.host().threads.get(&id).map(|thread| thread.read_seq),
        Some(0),
        "the refused read shows the thread unread again"
    );
    forge.host_mut().acknowledge_read(&id, 5);
    assert!(
        !forge.host().has_requests_in_flight(),
        "the same refused read is not sent again"
    );
}

#[test]
fn a_refused_read_is_sent_again_once_there_is_more_to_read() {
    let mut forge = StubbedHost::start(empty_state(), axum::Router::new());
    let id = unread(&mut forge, "chat");
    forge.host_mut().observe_thread(Some(id.clone()));
    forge.host_mut().acknowledge_read(&id, 5);
    forge
        .take("PATCH", "/threads/chat")
        .fail(StatusCode::BAD_REQUEST, "bad_request", "not now");
    read_answered(&mut forge);
    let mut newer = thread("chat", 4, 6);
    newer.read_seq = 0;
    publish_and_apply(&mut forge, vec![Event::Thread { thread: newer }]);
    forge.host_mut().acknowledge_read(&id, 6);
    let mut read = thread("chat", 5, 6);
    read.read_seq = 6;
    forge.take("PATCH", "/threads/chat").respond(read);
    read_answered(&mut forge);
    assert_eq!(
        forge.host().threads.get(&id).map(|thread| thread.read_seq),
        Some(6)
    );
}

#[test]
fn a_message_waits_only_for_permission_writes_queued_before_it() {
    let mut forge = StubbedHost::start(empty_state(), axum::Router::new());
    publish_and_apply(
        &mut forge,
        vec![Event::Thread {
            thread: thread("chat", 3, 3),
        }],
    );
    let id = ThreadId("chat".into());
    let _full = edit(
        &mut forge,
        "chat",
        ThreadEdit::Permissions(Permissions::Full),
    );
    let first = forge.take("PATCH", "/threads/chat");
    let gate = forge
        .host()
        .post_gate(&id)
        .expect("a permission write is queued");
    let _auto = edit(
        &mut forge,
        "chat",
        ThreadEdit::Permissions(Permissions::Auto),
    );
    let mut reply = thread("chat", 4, 3);
    reply.config.permissions = Permissions::Full;
    first.respond(reply);
    let _second = forge.take("PATCH", "/threads/chat");
    editor_core::runtime::runtime().block_on(async {
        tokio::time::timeout(std::time::Duration::from_secs(10), gate.opened())
            .await
            .expect("the gate opens once the writes queued before the message are done");
    });
}

#[test]
fn an_edit_refused_by_a_failed_creation_says_why_the_creation_failed() {
    use std::error::Error as _;
    let mut forge = StubbedHost::start(empty_state(), axum::Router::new());
    let id = ThreadId("new".into());
    let sent = new_thread(&forge, "new");
    forge.host_mut().begin_send(sent.clone());
    forge.host_mut().observe_thread(Some(id.clone()));
    forge.host_mut().settle_send(
        sent.attempt,
        editor_core::SendSettlement::HandedBack(editor_core::HandBack::Failed(
            editor_core::Problem::new("Couldn't send the message."),
        )),
    );
    let (sender, receiver) = mpsc::channel();
    let report: EditReport = Box::new(move |result| {
        sender
            .send(result.map_err(|error| error.source().map(ToString::to_string)))
            .expect("the test keeps the report receiver");
        Box::pin(async {})
    });
    forge
        .host_mut()
        .edit_thread(&id, ThreadEdit::Rename("named".into()), report);
    let cause = receiver.recv_timeout(std::time::Duration::from_secs(10));
    assert_eq!(
        cause.expect("the report arrives"),
        Err(Some(
            "the draft gave the message back: Couldn't send the message.".to_owned()
        ))
    );
}

#[test]
fn a_late_hand_back_from_an_earlier_attempt_leaves_the_retry_alone() {
    let mut forge = StubbedHost::start(empty_state(), axum::Router::new());
    let id = ThreadId("new".into());
    forge.host_mut().observe_thread(Some(id.clone()));
    let first = new_thread(&forge, "new");
    forge.host_mut().begin_send(first.clone());
    let retry = new_thread(&forge, "new");
    forge.host_mut().begin_send(retry.clone());
    hand_back(&mut forge, &first);
    assert!(
        forge.host().is_provisional(&id),
        "the retry is still creating the thread"
    );
    assert_eq!(
        forge
            .host()
            .local_messages(&id)
            .map(|item| item.id.clone())
            .collect::<Vec<_>>(),
        [retry.message],
        "the retry's bubble stays"
    );
    let renamed = edit(&mut forge, "new", ThreadEdit::Rename("named".into()));
    publish_and_apply(
        &mut forge,
        vec![Event::Thread {
            thread: thread("new", 4, 4),
        }],
    );
    forge
        .take("PATCH", "/threads/new")
        .respond(titled("new", 5, "named"));
    assert_eq!(
        reported(&mut forge, &renamed),
        Ok(()),
        "an edit queued on the retry is not refused because the earlier attempt failed"
    );
}

#[test]
fn retrying_a_first_message_whose_thread_forge_already_has_keeps_the_real_row() {
    let mut forge = StubbedHost::start(empty_state(), axum::Router::new());
    let id = ThreadId("new".into());
    forge.host_mut().observe_thread(Some(id.clone()));
    let first = new_thread(&forge, "new");
    forge.host_mut().begin_send(first.clone());
    publish_and_apply(
        &mut forge,
        vec![Event::Thread {
            thread: titled("new", 4, "named by Forge"),
        }],
    );
    hand_back(&mut forge, &first);
    let retry = new_thread(&forge, "new");
    forge.host_mut().begin_send(retry.clone());
    assert!(
        !forge.host().is_provisional(&id),
        "the retry does not turn Forge's thread back into a provisional row"
    );
    assert_eq!(title(&forge, "new").as_deref(), Some("named by Forge"));
    hand_back(&mut forge, &retry);
    forge.host_mut().observe_thread(None);
    assert_eq!(
        title(&forge, "new").as_deref(),
        Some("named by Forge"),
        "a failed retry leaves Forge's thread in the sidebar"
    );
}

#[test]
fn a_permission_change_made_after_sending_goes_out_after_the_message() {
    let mut forge = StubbedHost::start(empty_state(), axum::Router::new());
    publish_and_apply(
        &mut forge,
        vec![Event::Thread {
            thread: thread("chat", 3, 3),
        }],
    );
    let id = ThreadId("chat".into());
    let _full = edit(
        &mut forge,
        "chat",
        ThreadEdit::Permissions(Permissions::Full),
    );
    let full = forge.take("PATCH", "/threads/chat");
    let message = LocalSend {
        attempt: forge.host().send_receipt().attempt(),
        thread: id.clone(),
        message: artisan_api::ItemId("message".into()),
        text: "hello".into(),
        images: Vec::new(),
        create: None,
    };
    forge.host_mut().begin_send(message.clone());
    let _auto = edit(
        &mut forge,
        "chat",
        ThreadEdit::Permissions(Permissions::Auto),
    );
    let mut reply = thread("chat", 4, 3);
    reply.config.permissions = Permissions::Full;
    full.respond(reply);
    forge.settle("Full is saved", |core, host| {
        core.host(host).is_some_and(|data| {
            data.confirmed_thread(&ThreadId("chat".into()))
                .is_some_and(|thread| thread.config.permissions == Permissions::Full)
        })
    });
    assert!(
        forge.host().permissions_wait_for_send(&id),
        "Auto waits for the message sent before it, which carries Full"
    );
    forge
        .host_mut()
        .settle_send(message.attempt, editor_core::SendSettlement::Accepted);
    let auto = forge.take("PATCH", "/threads/chat");
    assert_eq!(
        auto.json::<serde_json::Value>()["config"]["permissions"],
        serde_json::json!("auto"),
        "the later choice is written after the message, so it wins"
    );
}

#[test]
fn a_failed_archive_returns_the_row_after_a_page_load_that_kept_it_in_view() {
    let mut forge = paged_history();
    for page in 0..6 {
        page_older(&mut forge, 10_000 - page * 200);
    }
    let row = "old-09000";
    let outcome = edit(&mut forge, row, ThreadEdit::Archive);
    let archive = forge.take("PATCH", &format!("/threads/{row}"));
    page_older(&mut forge, 8_800);
    archive.fail(StatusCode::INTERNAL_SERVER_ERROR, "internal", "disk full");
    assert!(reported(&mut forge, &outcome).is_err());
    assert!(
        inbox(&forge).contains(&row.to_owned()),
        "the row comes back to a window that still spans it"
    );
}

#[test]
fn a_message_gate_stays_open_once_its_writes_are_done_and_new_ones_start() {
    let mut forge = StubbedHost::start(empty_state(), axum::Router::new());
    publish_and_apply(
        &mut forge,
        vec![Event::Thread {
            thread: thread("chat", 3, 3),
        }],
    );
    let id = ThreadId("chat".into());
    let _full = edit(
        &mut forge,
        "chat",
        ThreadEdit::Permissions(Permissions::Full),
    );
    let gate = forge
        .host()
        .post_gate(&id)
        .expect("a permission write is queued");
    let mut reply = thread("chat", 4, 3);
    reply.config.permissions = Permissions::Full;
    forge.take("PATCH", "/threads/chat").respond(reply);
    forge.settle("Full is saved", |core, host| {
        core.host(host)
            .is_some_and(|data| !data.permissions_changing(&ThreadId("chat".into())))
    });
    let _auto = edit(
        &mut forge,
        "chat",
        ThreadEdit::Permissions(Permissions::Auto),
    );
    let _held = forge.take("PATCH", "/threads/chat");
    editor_core::runtime::runtime().block_on(async {
        tokio::time::timeout(std::time::Duration::from_secs(10), gate.opened())
            .await
            .expect("a later write on the thread does not close a gate whose writes are done");
    });
}
