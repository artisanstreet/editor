use artisan_api::{ItemId, ThreadId};
use axum::http::StatusCode;
use editor_aqua::conversation::adapter::{Action, ScopedAction};
use editor_aqua::conversation::controller::Conversation;
use editor_aqua::conversation::stored::StoredConversation;
use editor_aqua::core::{Core, HostConfig};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};
use test_support::editor::core::{connect, stub_host};
use test_support::editor::forge::StubForge;

fn forge() -> StubForge {
    let unavailable = axum::routing::any(|| async {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(serde_json::json!({"code": "internal", "message": "thread unavailable"})),
        )
    });
    editor_aqua::core::runtime::runtime().block_on(StubForge::serve(
        test_support::editor::forge::empty_state(),
        axum::Router::new().route("/threads/{*rest}", unavailable),
    ))
}

fn replace(core: &mut Core, config: &HostConfig) {
    core.remove_host(&config.id)
        .expect("the replaced host is removed");
    connect(core, config);
}

fn page_wakes(conversation: &Conversation) -> mpsc::Receiver<()> {
    let (wake, woken) = mpsc::channel();
    conversation
        .wake_mailbox()
        .expect("the conversation owns its wake mailbox")
        .set_waker(Arc::new(move || match wake.send(()) {
            Ok(()) | Err(mpsc::SendError(())) => {}
        }));
    woken
}

fn wait_for_page(woken: &mpsc::Receiver<()>) {
    woken
        .recv_timeout(Duration::from_secs(15))
        .expect("the page request delivers its completion");
}

#[test]
fn pending_page_rejects_removed_and_recreated_host() {
    let directory = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(directory.path());
    let forge = forge();
    let config = stub_host("same-host", "Forge", &forge);
    connect(&mut core, &config);
    let thread = ThreadId("thread".into());
    let mut conversation = Conversation::new(&core);
    let woken = page_wakes(&conversation);
    conversation.open_thread(&mut core, config.id.clone(), thread.clone());
    wait_for_page(&woken);
    conversation.drain(&mut core);
    assert!(
        conversation
            .load_error
            .as_deref()
            .is_some_and(|error| error.starts_with("Couldn't load this thread. "))
    );

    conversation.open_thread(&mut core, config.id.clone(), thread.clone());
    let original = core.host(&config.id).unwrap().instance();
    wait_for_page(&woken);
    core.remove_host(&config.id).unwrap();
    conversation.drain(&mut core);
    assert!(conversation.load_error.is_none());
    assert!(conversation.view.as_ref().unwrap().viewport.in_flight);

    connect(&mut core, &config);
    let replacement = core.host(&config.id).unwrap().instance();
    assert_ne!(original, replacement);
    conversation.open_thread(&mut core, config.id.clone(), thread);
    wait_for_page(&woken);
    replace(&mut core, &config);
    assert_ne!(replacement, core.host(&config.id).unwrap().instance());
    conversation.drain(&mut core);
    assert!(conversation.load_error.is_none());
    assert!(conversation.view.as_ref().unwrap().viewport.in_flight);
}

#[test]
fn retained_action_cannot_mutate_a_replacement_with_the_same_host_id() {
    let directory = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(directory.path());
    let forge = forge();
    let config = stub_host("same-host", "Forge", &forge);
    connect(&mut core, &config);
    let thread = ThreadId("thread".into());
    let mut conversation = Conversation::new(&core);
    conversation.open_thread(&mut core, config.id.clone(), thread.clone());
    let item = ItemId("item".into());
    let action = ScopedAction::new(
        &conversation,
        Action::ToggleItem {
            id: item.clone(),
            expanded: true,
        },
    )
    .unwrap();
    replace(&mut core, &config);
    assert!(!action.dispatch(
        &mut conversation,
        &mut StoredConversation::default(),
        &core,
        true,
        Instant::now()
    ));
    assert!(!conversation.view.as_ref().unwrap().ui.contains_key(&item));

    let old_action = ScopedAction::new(
        &conversation,
        Action::ToggleItem {
            id: item.clone(),
            expanded: true,
        },
    )
    .unwrap();
    conversation.open_thread(&mut core, config.id.clone(), thread);
    assert!(!old_action.dispatch(
        &mut conversation,
        &mut StoredConversation::default(),
        &core,
        true,
        Instant::now()
    ));
    assert!(!conversation.view.as_ref().unwrap().ui.contains_key(&item));
    let current = ScopedAction::new(
        &conversation,
        Action::ToggleItem {
            id: item.clone(),
            expanded: true,
        },
    )
    .unwrap();
    assert!(current.dispatch(
        &mut conversation,
        &mut StoredConversation::default(),
        &core,
        true,
        Instant::now()
    ));
    assert_eq!(
        conversation.view.as_ref().unwrap().ui[&item].expanded,
        Some(true)
    );
    let repeated = ScopedAction::new(
        &conversation,
        Action::ToggleItem {
            id: item,
            expanded: true,
        },
    )
    .unwrap();
    assert!(!repeated.dispatch(
        &mut conversation,
        &mut StoredConversation::default(),
        &core,
        true,
        Instant::now()
    ));
}
