use super::*;
use artisan_api::{EngineConfig, EngineId, Event, ProjectId, Status, Thread};
use editor_core::{LocalSend, NewThread};
use test_support::editor::core::StubbedHost;
use test_support::editor::forge::empty_state;

fn message(id: &str, position: Seq) -> Item {
    Item {
        id: ItemId(id.into()),
        thread_id: ThreadId("new".into()),
        position,
        seq: position,
        run_id: None,
        parent_id: None,
        status: Status::Done,
        created_at: 0,
        updated_at: 0,
        kind: ItemKind::UserMessage {
            text: id.into(),
            images: Vec::new(),
            steer: false,
            from_agent: None,
            sender: None,
        },
    }
}

fn created() -> Thread {
    serde_json::from_value(serde_json::json!({
        "id": "new",
        "project_id": "project",
        "title": null,
        "config": {"engine": "claude"},
        "activity_seq": 4,
        "read_seq": 4,
        "seq": 4,
        "created_at": 0,
        "updated_at": 0,
        "archived": false,
    }))
    .expect("thread")
}

fn rows_for(conversation: &Conversation, id: &str) -> usize {
    conversation
        .rows
        .iter()
        .filter(|(row, _)| row.anchor() == Some(&ItemId(id.into())))
        .count()
}

fn pump(forge: &mut StubbedHost, conversation: &mut Conversation) {
    forge
        .core
        .drain_with(|core, event| conversation.core_event(core, &event, true));
    conversation.drain(&mut forge.core);
}

#[test]
fn a_thread_opened_on_send_shows_the_message_at_once_and_loads_once_forge_has_it() {
    let mut forge = StubbedHost::start(empty_state(), axum::Router::new());
    let thread = ThreadId("new".into());
    let attempt = forge.host().send_receipt().attempt();
    forge.host_mut().begin_send(LocalSend {
        attempt,
        thread: thread.clone(),
        message: ItemId("hello".into()),
        text: "hello".into(),
        images: Vec::new(),
        create: Some(NewThread {
            project: ProjectId("project".into()),
            config: EngineConfig::new(EngineId::Claude),
        }),
    });
    let mut conversation = Conversation::new(&forge.core);
    let host = forge.host.clone();
    conversation.open_thread(&mut forge.core, host, thread);
    assert!(
        !conversation.is_landing(),
        "the new thread shows straight away instead of a loader"
    );
    assert_eq!(
        rows_for(&conversation, "hello"),
        1,
        "the sent message shows"
    );
    assert!(
        conversation.page_task.is_none(),
        "nothing is asked of Forge before it has the thread"
    );

    forge.forge.publish(Event::Thread { thread: created() });
    let transcript = test_support::wait::wait_for(
        "the transcript to be asked for",
        std::time::Duration::from_secs(10),
        || {
            pump(&mut forge, &mut conversation);
            forge.forge.try_take("GET", "/threads/new/transcript")
        },
    );
    assert_eq!(
        rows_for(&conversation, "hello"),
        1,
        "the message stays while the transcript loads"
    );
    transcript.respond(ItemsPage {
        items: Vec::new(),
        runs: Vec::new(),
        more: false,
        work: Vec::new(),
    });
    forge.forge.publish(Event::Item {
        item: message("hello", 5),
    });
    test_support::wait::wait_until(
        "Forge's message to arrive",
        std::time::Duration::from_secs(10),
        || {
            pump(&mut forge, &mut conversation);
            conversation
                .view
                .as_ref()
                .and_then(|view| view.item(&ItemId("hello".into())))
                .is_some_and(|item| item.seq == 5)
        },
    );
    assert_eq!(
        rows_for(&conversation, "hello"),
        1,
        "Forge's message replaces the local one without a duplicate"
    );
}

#[test]
fn a_message_streamed_before_the_first_page_lands_survives_that_page() {
    let mut forge = StubbedHost::start(empty_state(), axum::Router::new());
    let thread = ThreadId("new".into());
    forge.forge.publish(Event::Thread { thread: created() });
    forge.apply_published();
    let mut conversation = Conversation::new(&forge.core);
    let host = forge.host.clone();
    conversation.open_thread(&mut forge.core, host, thread);
    let transcript = test_support::wait::wait_for(
        "the transcript to be asked for",
        std::time::Duration::from_secs(10),
        || {
            pump(&mut forge, &mut conversation);
            forge.forge.try_take("GET", "/threads/new/transcript")
        },
    );
    forge.forge.publish(Event::Item {
        item: message("hello", 5),
    });
    test_support::wait::wait_until(
        "the streamed message to show while the page loads",
        std::time::Duration::from_secs(10),
        || {
            pump(&mut forge, &mut conversation);
            conversation
                .view
                .as_ref()
                .and_then(|view| view.item(&ItemId("hello".into())))
                .is_some()
        },
    );
    transcript.respond(ItemsPage {
        items: Vec::new(),
        runs: Vec::new(),
        more: false,
        work: Vec::new(),
    });
    test_support::wait::wait_until(
        "the page Forge read before the message to land",
        std::time::Duration::from_secs(10),
        || {
            pump(&mut forge, &mut conversation);
            conversation.page_task.is_none()
        },
    );
    assert_eq!(
        conversation
            .view
            .as_ref()
            .and_then(|view| view.item(&ItemId("hello".into())))
            .map(|item| item.seq),
        Some(5),
        "the page, read before the message existed, does not erase it"
    );
    assert_eq!(rows_for(&conversation, "hello"), 1);
}
