use crate::conversation::controller::Conversation;
use crate::conversation::mentions::TITLE_RECHECK;
use crate::conversation::stored::StoredConversation;
use crate::conversation::view::factory::realize::harness::{Transcript, deliver};
use artisan_api::{Event, ThreadId};
use editor_core::{CoreEvent, HostEvent, Screen};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use test_support::editor::core::StubbedHost;
use test_support::editor::forge::empty_state;
use test_support::items::{item, user_message};

const ASK: &str = "Ask @Sidebar fix about it";

fn thread(id: &str, title: &str) -> artisan_api::Thread {
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

fn titled(stub: &mut StubbedHost, title: &str) {
    stub.host_mut()
        .threads
        .insert(ThreadId("other".into()), thread("other", title));
}

#[test]
fn a_rename_inside_the_recheck_window_wakes_the_transcript_which_shows_it() {
    let timers = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .expect("a paused timer runtime");
    let mut stub = StubbedHost::start(empty_state(), axum::Router::new());
    titled(&mut stub, "Sidebar lag");
    let own = ThreadId("open".into());
    let host = stub.host.clone();
    stub.core.navigate(Screen::Thread {
        host: host.clone(),
        thread: own.clone(),
    });
    let mut conversation = Conversation::new(&stub.core);
    conversation.open_thread(&mut stub.core, host, own.clone());
    stub.forge.publish(Event::Item {
        item: item("ask", own, 1, user_message(ASK)),
    });
    deliver(
        &mut stub.core,
        &mut conversation,
        "the user message",
        |event| matches!(event, CoreEvent::Host { event: HostEvent::Item(item), .. } if item.id.0 == "ask"),
    );
    let stored = Rc::new(RefCell::new(StoredConversation::with_timers(
        timers.handle().clone(),
    )));
    let mut transcript =
        Transcript::open(&conversation, &stub.core).with_stored(Rc::clone(&stored));
    let shows = |transcript: &Transcript, title: &str| {
        transcript
            .markdown()
            .borrow()
            .vocabulary()
            .mentions(&format!("@{title}"))
            .len()
            == 1
    };

    let start = Instant::now();
    transcript.render_at(&mut conversation, &stub.core, start);
    assert!(shows(&transcript, "Sidebar lag"));
    assert_eq!(
        transcript.glyphs_in(ASK),
        0,
        "the message names no thread yet"
    );
    titled(&mut stub, "Sidebar fix");
    transcript.render_at(
        &mut conversation,
        &stub.core,
        start + Duration::from_millis(500),
    );
    assert!(!shows(&transcript, "Sidebar fix"));
    assert!(
        !stored
            .borrow_mut()
            .drain_publish(&mut conversation, &stub.core)
    );

    timers.block_on(async { tokio::time::sleep(TITLE_RECHECK).await });
    assert!(
        stored
            .borrow_mut()
            .drain_publish(&mut conversation, &stub.core),
        "the recheck wake asks for a transcript refresh"
    );
    transcript.render_at(&mut conversation, &stub.core, start + TITLE_RECHECK);
    assert!(shows(&transcript, "Sidebar fix"));
    assert_eq!(
        transcript.glyphs_in(ASK),
        1,
        "the message chips the renamed thread"
    );
    assert!(
        !stored
            .borrow_mut()
            .drain_publish(&mut conversation, &stub.core)
    );
}
