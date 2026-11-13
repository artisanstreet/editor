use super::*;
use artisan_api::Status;
use editor_core::{EditorFiles, ForgeClient, HostConfig, HostKind};

const HOST: &str = "switch-host";

fn fixture() -> (tempfile::TempDir, std::net::TcpListener, Core, Conversation) {
    let directory = tempfile::tempdir().unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: directory.path().to_owned(),
    })
    .unwrap();
    let config = HostConfig {
        id: HostId(HOST.into()),
        name: "Switch Forge".into(),
        address: listener.local_addr().unwrap().to_string(),
        fingerprint: "0".repeat(64),
        kind: HostKind::Local,
    };
    let client = ForgeClient::new(
        &config.address,
        &config.fingerprint,
        core.identity.as_ref().unwrap(),
    )
    .unwrap();
    core.add_test_host(&config, client);
    let mut conversation = Conversation::new(&core);
    conversation.open_thread(&mut core, config.id, ThreadId("first".into()));
    (directory, listener, core, conversation)
}

fn page(thread: &str) -> ItemsPage {
    ItemsPage {
        items: vec![Item {
            id: ItemId(format!("{thread}-item")),
            thread_id: ThreadId(thread.into()),
            position: 1,
            seq: 1,
            run_id: None,
            parent_id: None,
            status: Status::Done,
            created_at: 0,
            updated_at: 0,
            kind: ItemKind::UserMessage {
                text: thread.into(),
                images: Vec::new(),
                steer: false,
                from_agent: None,
                sender: None,
            },
        }],
        runs: Vec::new(),
        more: false,
        work: Vec::new(),
    }
}

fn deliver(conversation: &mut Conversation, core: &mut Core, completion: Completion) {
    assert!(
        runtime()
            .block_on(conversation.delivery.sender().send(completion))
            .is_ok()
    );
    assert!(conversation.drain(core));
}

fn page_for(origin: Origin, fetch: Fetch, thread: &str) -> Completion {
    Completion::Page {
        origin,
        fetch,
        asked: Instant::now(),
        result: Ok(page(thread)),
    }
}

fn current(conversation: &Conversation) -> Origin {
    Origin {
        host: conversation.open.as_ref().unwrap().0.clone(),
        instance: conversation.host_instance.unwrap(),
        generation: conversation.generation,
    }
}

fn held(conversation: &Conversation) -> Origin {
    conversation.pending.as_ref().unwrap().origin.clone()
}

fn land(conversation: &mut Conversation, core: &mut Core, thread: &str) {
    let origin = current(conversation);
    deliver(conversation, core, page_for(origin, Fetch::Tail, thread));
    conversation.landed();
}

fn shown(conversation: &Conversation) -> &str {
    conversation.open.as_ref().unwrap().1.as_str()
}

fn holds(conversation: &Conversation, thread: &str) -> bool {
    conversation
        .view
        .as_ref()
        .is_some_and(|view| view.item(&ItemId(format!("{thread}-item"))).is_some())
}

#[test]
fn a_switch_keeps_the_settled_thread_on_screen_until_the_next_page_arrives() {
    let (_directory, _listener, mut core, mut conversation) = fixture();
    land(&mut conversation, &mut core, "first");

    conversation.open_thread(&mut core, HostId(HOST.into()), ThreadId("second".into()));

    assert_eq!(shown(&conversation), "first");
    assert!(holds(&conversation, "first"));
    assert!(!conversation.is_landing());

    let origin = held(&conversation);
    deliver(
        &mut conversation,
        &mut core,
        page_for(origin, Fetch::Tail, "second"),
    );

    assert_eq!(shown(&conversation), "second");
    assert!(holds(&conversation, "second"));
    assert!(conversation.pending.is_none());
    assert!(matches!(
        conversation.landing,
        Some(Landing::Settling { .. })
    ));
}

#[test]
fn a_slow_switch_falls_back_to_loading_and_keeps_the_request_in_flight() {
    let (_directory, _listener, mut core, mut conversation) = fixture();
    land(&mut conversation, &mut core, "first");
    conversation.open_thread(&mut core, HostId(HOST.into()), ThreadId("second".into()));

    let origin = held(&conversation);
    deliver(
        &mut conversation,
        &mut core,
        Completion::Held {
            origin: origin.clone(),
        },
    );

    assert_eq!(shown(&conversation), "second");
    assert_eq!(conversation.landing, Some(Landing::Loading));
    assert!(conversation.page_task.is_some());

    deliver(
        &mut conversation,
        &mut core,
        page_for(origin, Fetch::Tail, "second"),
    );
    assert!(holds(&conversation, "second"));
}

#[test]
fn going_back_before_the_page_arrives_cancels_the_switch() {
    let (_directory, _listener, mut core, mut conversation) = fixture();
    land(&mut conversation, &mut core, "first");
    conversation.open_thread(&mut core, HostId(HOST.into()), ThreadId("second".into()));
    let origin = held(&conversation);

    conversation.open_thread(&mut core, HostId(HOST.into()), ThreadId("first".into()));

    assert!(conversation.pending.is_none());
    assert!(
        runtime()
            .block_on(
                conversation
                    .delivery
                    .sender()
                    .send(page_for(origin, Fetch::Tail, "second"))
            )
            .is_ok()
    );
    assert!(!conversation.drain(&mut core));
    assert_eq!(shown(&conversation), "first");
    assert!(holds(&conversation, "first"));
}

#[test]
fn returning_to_a_parked_thread_shows_it_at_once_and_refreshes_it() {
    let (_directory, _listener, mut core, mut conversation) = fixture();
    land(&mut conversation, &mut core, "first");
    conversation.open_thread(&mut core, HostId(HOST.into()), ThreadId("second".into()));
    let origin = held(&conversation);
    deliver(
        &mut conversation,
        &mut core,
        page_for(origin, Fetch::Tail, "second"),
    );
    conversation.landed();
    conversation.scroll_actions();

    conversation.open_thread(&mut core, HostId(HOST.into()), ThreadId("first".into()));

    assert_eq!(shown(&conversation), "first");
    assert!(holds(&conversation, "first"));
    assert!(!conversation.is_landing());
    assert!(!conversation.rows.is_empty());
    assert_eq!(
        conversation.view.as_ref().unwrap().viewport.requested,
        Some(Fetch::Refresh)
    );
    assert_eq!(conversation.scroll_actions(), [ScrollAction::Tail]);
}

#[test]
fn a_reset_forgets_the_parked_threads_of_its_host() {
    let (_directory, _listener, mut core, mut conversation) = fixture();
    land(&mut conversation, &mut core, "first");
    conversation.open_thread(&mut core, HostId(HOST.into()), ThreadId("second".into()));
    let origin = held(&conversation);
    deliver(
        &mut conversation,
        &mut core,
        page_for(origin, Fetch::Tail, "second"),
    );
    assert_eq!(conversation.parked.len(), 1);

    conversation.host_event(&core, &HostEvent::Reset, false);

    assert!(conversation.parked.is_empty());
}
