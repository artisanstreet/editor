use artisan_api::{Event, ThreadId};
use axum::http::StatusCode;
use test_support::editor::core::StubbedHost;
use test_support::editor::forge::{HeldRequest, empty_state};

fn thread(id: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "project_id": "project",
        "title": id,
        "config": {"engine": "claude"},
        "activity_seq": 5,
        "read_seq": 0,
        "seq": 3,
        "created_at": 0,
        "updated_at": 0,
        "archived": false,
    })
}

fn read_thread(id: &str) -> serde_json::Value {
    let mut updated = thread(id);
    updated["seq"] = serde_json::json!(4);
    updated["read_seq"] = serde_json::json!(5);
    updated
}

fn forge() -> StubbedHost {
    StubbedHost::start(empty_state(), axum::Router::new())
}

fn has_thread(stubbed: &StubbedHost, id: &str) -> bool {
    stubbed.host().threads.contains_key(&ThreadId(id.into()))
}

fn read(stubbed: &mut StubbedHost, id: &str) {
    let thread_id = ThreadId(id.into());
    stubbed.host_mut().observe_thread(Some(thread_id.clone()));
    stubbed.forge.publish(Event::Thread {
        thread: serde_json::from_value(thread(id)).expect("thread"),
    });
    stubbed.settle("the observed thread arrives", |core, host| {
        core.host(host)
            .is_some_and(|data| data.threads.contains_key(&thread_id))
    });
    stubbed.host_mut().acknowledge_read(&thread_id, 5);
}

fn acknowledgement(stubbed: &mut StubbedHost, id: &str) -> HeldRequest {
    stubbed.take("PATCH", &format!("/threads/{id}"))
}

fn delete(stubbed: &mut StubbedHost, id: &str) {
    stubbed.forge.publish(Event::ThreadRemoved {
        thread_id: ThreadId(id.into()),
    });
    stubbed.settle("the deletion applies", |core, host| {
        core.host(host)
            .is_some_and(|data| !data.threads.contains_key(&ThreadId(id.into())))
    });
}

#[test]
fn a_thread_forge_no_longer_has_does_not_block_other_read_acknowledgements() {
    let mut forge = forge();
    read(&mut forge, "gone");
    let gone = acknowledgement(&mut forge, "gone");
    read(&mut forge, "kept");
    gone.fail(StatusCode::NOT_FOUND, "not_found", "no such thread");
    acknowledgement(&mut forge, "kept").respond(read_thread("kept"));
}

#[test]
fn deleting_a_thread_cancels_its_waiting_read_acknowledgement() {
    let mut forge = forge();
    read(&mut forge, "held");
    let _held = acknowledgement(&mut forge, "held");
    delete(&mut forge, "held");
    read(&mut forge, "kept");
    acknowledgement(&mut forge, "kept").respond(read_thread("kept"));
}

#[test]
fn a_read_acknowledgement_answered_after_a_delete_keeps_the_thread_deleted() {
    let mut forge = forge();
    read(&mut forge, "held");
    let held = acknowledgement(&mut forge, "held");
    delete(&mut forge, "held");
    held.respond(read_thread("held"));
    read(&mut forge, "kept");
    acknowledgement(&mut forge, "kept").respond(read_thread("kept"));
    forge.settle("the later acknowledgement settles", |core, host| {
        core.host(host)
            .and_then(|data| data.threads.get(&ThreadId("kept".into())))
            .is_some_and(|thread| thread.seq == 4)
    });
    assert!(
        !has_thread(&forge, "held"),
        "a late acknowledgement does not bring the deleted thread back"
    );
}

fn unread(stubbed: &StubbedHost, id: &str) -> bool {
    stubbed
        .host()
        .threads
        .get(&ThreadId(id.into()))
        .is_some_and(artisan_api::Thread::is_unread)
}

#[test]
fn a_read_thread_shows_read_at_once_and_unread_again_when_forge_refuses() {
    let mut forge = forge();
    read(&mut forge, "chat");
    assert!(
        !unread(&forge, "chat"),
        "the thread shows read before Forge answers"
    );
    acknowledgement(&mut forge, "chat").fail(StatusCode::NOT_FOUND, "not_found", "no such thread");
    forge.settle("the refused acknowledgement rolls back", |core, host| {
        core.host(host)
            .and_then(|data| data.threads.get(&ThreadId("chat".into())))
            .is_some_and(artisan_api::Thread::is_unread)
    });
}
