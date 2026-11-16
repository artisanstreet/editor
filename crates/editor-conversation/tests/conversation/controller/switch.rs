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
fn going_back_before_the_page_arrives_cancels_the_switch_and_lands_at_the_latest_message() {
    let (_directory, _listener, mut core, mut conversation) = fixture();
    land(&mut conversation, &mut core, "first");
    conversation.open_thread(&mut core, HostId(HOST.into()), ThreadId("second".into()));
    let origin = held(&conversation);
    conversation.scroll_actions();

    conversation.open_thread(&mut core, HostId(HOST.into()), ThreadId("first".into()));

    assert!(conversation.pending.is_none());
    assert_eq!(conversation.scroll_actions(), [ScrollAction::Tail]);
    assert!(conversation.landed_at.is_some());
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
fn returning_to_a_parked_thread_fades_it_in_at_once_and_refreshes_it() {
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
    assert!(conversation.landed_at.is_some());
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

#[test]
fn returning_to_a_parked_thread_whose_latest_rows_were_evicted_waits_for_them() {
    let (_directory, _listener, mut core, mut conversation) = fixture();
    land(&mut conversation, &mut core, "first");
    conversation.view.as_mut().unwrap().viewport.gap = Some(1);
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

    assert!(conversation.is_landing());
    assert_eq!(conversation.reveal(Instant::now()), 0.0);
    assert!(conversation.scroll_actions().is_empty());
    let origin = current(&conversation);
    deliver(
        &mut conversation,
        &mut core,
        page_for(origin, Fetch::Tail, "first"),
    );
    assert_eq!(conversation.scroll_actions(), [ScrollAction::End]);
    assert!(matches!(
        super::super::adapter::scroll_directive(&conversation, ScrollAction::End, false),
        Some(super::super::adapter::ScrollDirective::FollowEnd)
    ));
}

#[test]
fn coming_back_while_evicted_rows_load_keeps_the_request_in_flight() {
    let (_directory, _listener, mut core, mut conversation) = fixture();
    land(&mut conversation, &mut core, "first");
    conversation.view.as_mut().unwrap().viewport.gap = Some(1);
    conversation.open_thread(&mut core, HostId(HOST.into()), ThreadId("second".into()));
    let origin = held(&conversation);
    deliver(
        &mut conversation,
        &mut core,
        page_for(origin, Fetch::Tail, "second"),
    );
    let thread = Screen::Thread {
        host: HostId(HOST.into()),
        thread: ThreadId("first".into()),
    };
    core.navigate(thread.clone());
    conversation.sync_screen(&mut core);
    let asked = conversation
        .page_task
        .as_ref()
        .map(|task| task.abort_handle().id());
    assert!(asked.is_some());

    core.navigate(Screen::Usage);
    conversation.sync_screen(&mut core);
    core.navigate(thread);
    conversation.sync_screen(&mut core);

    assert_eq!(
        conversation
            .page_task
            .as_ref()
            .map(|task| task.abort_handle().id()),
        asked
    );
    assert_eq!(conversation.landing, Some(Landing::Loading));
}

#[test]
fn coming_back_from_another_screen_lands_the_open_thread_at_the_latest_message() {
    let (_directory, _listener, mut core, mut conversation) = fixture();
    land(&mut conversation, &mut core, "first");
    conversation.viewport(
        &core,
        Input::ScrolledAway {
            anchor: 1,
            through: 1,
        },
    );
    conversation.scroll_actions();
    let thread = Screen::Thread {
        host: HostId(HOST.into()),
        thread: ThreadId("first".into()),
    };
    core.navigate(thread.clone());
    conversation.sync_screen(&mut core);
    assert!(conversation.scroll_actions().is_empty());

    core.navigate(Screen::Usage);
    conversation.sync_screen(&mut core);
    core.navigate(thread);
    conversation.sync_screen(&mut core);

    assert_eq!(conversation.scroll_actions(), [ScrollAction::Tail]);
    assert!(
        conversation
            .view
            .as_ref()
            .unwrap()
            .viewport
            .state()
            .is_following()
    );
    assert!(conversation.landed_at.is_some());
}

#[test]
fn a_thread_too_short_to_scroll_follows_new_messages() {
    let (_directory, _listener, mut core, mut conversation) = fixture();
    land(&mut conversation, &mut core, "first");

    let inputs =
        super::super::scroll::scrolled(&conversation, 0, 600.0, 0.0, 0.0, false, false, |_| false);

    assert!(
        inputs
            .iter()
            .any(|input| matches!(input, Input::BottomReached))
    );
}

#[test]
fn coming_back_while_the_first_page_loads_keeps_waiting_for_it() {
    let (_directory, _listener, mut core, mut conversation) = fixture();
    let thread = Screen::Thread {
        host: HostId(HOST.into()),
        thread: ThreadId("first".into()),
    };
    core.navigate(thread.clone());
    conversation.sync_screen(&mut core);

    core.navigate(Screen::Usage);
    conversation.sync_screen(&mut core);
    core.navigate(thread);
    conversation.sync_screen(&mut core);

    assert_eq!(conversation.landing, Some(Landing::Loading));
    assert!(conversation.landed_at.is_none());
    let origin = current(&conversation);
    deliver(
        &mut conversation,
        &mut core,
        page_for(origin, Fetch::Tail, "first"),
    );
    assert!(conversation.is_landing());
    let actions = conversation.scroll_actions();
    assert!(!actions.is_empty());
    assert!(actions.iter().all(|action| matches!(
        super::super::adapter::scroll_directive(&conversation, action.clone(), false),
        Some(super::super::adapter::ScrollDirective::FollowEnd)
    )));
}
