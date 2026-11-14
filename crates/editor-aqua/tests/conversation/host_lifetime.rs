use artisan_api::{ItemId, ThreadId};
use editor_aqua::conversation::adapter::{Action, ScopedAction};
use editor_aqua::conversation::controller::Conversation;
use editor_aqua::core::{Core, EditorFiles, ForgeClient, HostConfig, HostId, HostKind};
use std::time::{Duration, Instant};

fn config() -> HostConfig {
    HostConfig {
        id: HostId("same-host".into()),
        name: "Forge".into(),
        address: "127.0.0.1:1".into(),
        fingerprint: "0".repeat(64),
        kind: HostKind::Remote,
    }
}

fn client(core: &Core) -> ForgeClient {
    ForgeClient::new(
        "127.0.0.1:1",
        &"0".repeat(64),
        core.identity.as_ref().unwrap(),
    )
    .unwrap()
}

fn wait_for_page(conversation: &Conversation) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !conversation.page_task_finished() {
        assert!(Instant::now() < deadline, "page request did not finish");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn pending_page_rejects_removed_and_recreated_host() {
    let directory = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: directory.path().to_owned(),
    })
    .unwrap();
    let config = config();
    core.add_test_host(&config, client(&core));
    let thread = ThreadId("thread".into());
    let mut conversation = Conversation::new(&core);
    conversation.open_thread(&mut core, config.id.clone(), thread.clone());
    wait_for_page(&conversation);
    conversation.drain(&mut core);
    assert!(
        conversation
            .load_error
            .as_deref()
            .is_some_and(|error| error.starts_with("Couldn't load this thread:"))
    );

    conversation.open_thread(&mut core, config.id.clone(), thread.clone());
    let original = core.host(&config.id).unwrap().instance();
    wait_for_page(&conversation);
    core.remove_host(&config.id).unwrap();
    conversation.drain(&mut core);
    assert!(conversation.load_error.is_none());
    assert!(conversation.view.as_ref().unwrap().viewport.in_flight);

    core.add_test_host(&config, client(&core));
    let replacement = core.host(&config.id).unwrap().instance();
    assert_ne!(original, replacement);
    conversation.open_thread(&mut core, config.id.clone(), thread);
    wait_for_page(&conversation);
    core.add_test_host(&config, client(&core));
    assert_ne!(replacement, core.host(&config.id).unwrap().instance());
    conversation.drain(&mut core);
    assert!(conversation.load_error.is_none());
    assert!(conversation.view.as_ref().unwrap().viewport.in_flight);
}

#[test]
fn retained_action_cannot_mutate_a_replacement_with_the_same_host_id() {
    let directory = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: directory.path().to_owned(),
    })
    .unwrap();
    let config = config();
    core.add_test_host(&config, client(&core));
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
    core.add_test_host(&config, client(&core));
    assert!(!action.dispatch(&mut conversation, &core, true, Instant::now()));
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
    assert!(!old_action.dispatch(&mut conversation, &core, true, Instant::now()));
    assert!(!conversation.view.as_ref().unwrap().ui.contains_key(&item));
    let current = ScopedAction::new(
        &conversation,
        Action::ToggleItem {
            id: item.clone(),
            expanded: true,
        },
    )
    .unwrap();
    assert!(current.dispatch(&mut conversation, &core, true, Instant::now()));
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
    assert!(!repeated.dispatch(&mut conversation, &core, true, Instant::now()));
}
