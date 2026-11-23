use artisan_api::{
    EngineConfig, EngineId, Event, Item, ItemId, ItemKind, ProjectId, Status, Thread, ThreadId,
};
use editor_core::{HandBack, LocalSend, NewThread, SendSettlement};
use test_support::editor::core::StubbedHost;
use test_support::editor::forge::empty_state;

fn forge() -> StubbedHost {
    StubbedHost::start(empty_state(), axum::Router::new())
}

fn send(forge: &StubbedHost, thread: &str, message: &str, create: bool) -> LocalSend {
    LocalSend {
        attempt: forge.host().send_receipt().attempt(),
        thread: ThreadId(thread.into()),
        message: ItemId(message.into()),
        text: message.into(),
        images: Vec::new(),
        create: create.then(|| NewThread {
            project: ProjectId("project".into()),
            config: EngineConfig::new(EngineId::Claude),
        }),
    }
}

fn forge_thread(id: &str, seq: u64) -> Thread {
    serde_json::from_value(serde_json::json!({
        "id": id,
        "project_id": "project",
        "title": null,
        "config": {"engine": "claude"},
        "activity_seq": seq,
        "read_seq": seq,
        "seq": seq,
        "created_at": 0,
        "updated_at": 0,
        "archived": false,
    }))
    .expect("thread")
}

fn forge_message(thread: &str, id: &str, seq: u64) -> Item {
    Item {
        id: ItemId(id.into()),
        thread_id: ThreadId(thread.into()),
        position: seq,
        seq,
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

fn local(forge: &StubbedHost, thread: &str) -> Vec<String> {
    forge
        .host()
        .local_messages(&ThreadId(thread.into()))
        .map(|item| item.id.0.clone())
        .collect()
}

fn listed(forge: &StubbedHost) -> Vec<String> {
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

#[test]
fn a_new_thread_and_its_message_show_before_forge_answers_and_give_way_to_forge() {
    let mut forge = forge();
    publish_and_apply(
        &mut forge,
        vec![Event::Thread {
            thread: forge_thread("older", 7),
        }],
    );
    let hello = send(&forge, "new", "hello", true);
    forge.host_mut().begin_send(hello);
    assert_eq!(
        listed(&forge),
        ["new", "older"],
        "the new thread heads the list at once"
    );
    assert!(forge.host().is_provisional(&ThreadId("new".into())));
    assert_eq!(local(&forge, "new"), ["hello"], "the message shows at once");

    publish_and_apply(
        &mut forge,
        vec![
            Event::Thread {
                thread: forge_thread("new", 8),
            },
            Event::Item {
                item: forge_message("new", "hello", 9),
            },
        ],
    );
    assert!(
        !forge.host().is_provisional(&ThreadId("new".into())),
        "Forge's copy replaces the provisional thread"
    );
    assert_eq!(listed(&forge), ["new", "older"]);
    assert!(
        local(&forge, "new").is_empty(),
        "Forge's message replaces the local one"
    );
}

#[test]
fn a_failed_new_thread_stays_until_the_user_leaves_it() {
    let mut forge = forge();
    let thread = ThreadId("new".into());
    forge.host_mut().observe_thread(Some(thread.clone()));
    let hello = send(&forge, "new", "hello", true);
    forge.host_mut().begin_send(hello.clone());
    forge
        .host_mut()
        .settle_send(hello.attempt, SendSettlement::HandedBack(HandBack::Closed));
    assert!(
        local(&forge, "new").is_empty(),
        "the message goes back to the composer"
    );
    assert!(
        forge.host().threads.contains_key(&thread),
        "the thread view stays while the user is on it"
    );
    forge.host_mut().observe_thread(None);
    assert!(
        !forge.host().threads.contains_key(&thread),
        "leaving a thread Forge never created removes it"
    );
    assert!(listed(&forge).is_empty());
}

#[test]
fn a_reconnect_keeps_unanswered_messages_and_drops_ones_forge_accepted() {
    let mut forge = forge();
    publish_and_apply(
        &mut forge,
        vec![Event::Thread {
            thread: forge_thread("chat", 3),
        }],
    );
    let accepted = send(&forge, "chat", "accepted", false);
    forge.host_mut().begin_send(accepted.clone());
    let waiting = send(&forge, "chat", "waiting", false);
    forge.host_mut().begin_send(waiting);
    forge
        .host_mut()
        .settle_send(accepted.attempt, SendSettlement::Accepted);
    forge.forge.publish(Event::Reset);
    forge.settle("Forge's state reloads", |core, host| {
        core.host(host)
            .is_some_and(|data| data.local_messages(&ThreadId("chat".into())).count() == 1)
    });
    assert_eq!(
        local(&forge, "chat"),
        ["waiting"],
        "the reloaded transcript carries the accepted message, the unanswered one stays"
    );
}
