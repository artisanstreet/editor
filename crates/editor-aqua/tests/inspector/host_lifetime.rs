use artisan_api::{ItemId, ThreadId};
use editor_aqua::{
    core::{Core, HostConfig, HostId, HostKind},
    inspector::TerminalDialog,
};
use std::sync::{Arc, mpsc};
use std::time::Duration;
use test_support::net::ClosingListener;

fn config(forge: &ClosingListener) -> HostConfig {
    HostConfig {
        id: HostId("same-host".into()),
        name: "Forge".into(),
        address: forge.address().to_string(),
        fingerprint: "0".repeat(64),
        kind: HostKind::Remote,
    }
}

fn pending_dialog(core: &Core, config: &HostConfig) -> TerminalDialog {
    let mut dialog = TerminalDialog::default();
    let (wake, woken) = mpsc::channel();
    dialog
        .wake_mailbox()
        .expect("the terminal dialog owns its wake mailbox")
        .set_waker(Arc::new(move || match wake.send(()) {
            Ok(()) | Err(mpsc::SendError(())) => {}
        }));
    let open = (config.id.clone(), ThreadId("thread".into()));
    assert!(dialog.open(core, Some(&open), &ItemId("item".into())));
    woken
        .recv_timeout(Duration::from_secs(15))
        .expect("the terminal fetch delivers its completion");
    dialog
}

#[test]
fn terminal_fetch_completion_is_discarded_after_removal_and_replacement() {
    let directory = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(directory.path());
    let forge = ClosingListener::bind();
    let config = config(&forge);
    test_support::editor::core::connect(&mut core, &config);
    let mut current = pending_dialog(&core, &config);
    assert!(current.drain(&core));
    assert!(current.error().is_some_and(|error| {
        error
            .headline()
            .starts_with("Couldn't load the shell output.")
    }));

    let mut removed = pending_dialog(&core, &config);
    let old = core.host(&config.id).unwrap().instance();
    core.remove_host(&config.id).unwrap();
    assert!(removed.drain(&core));
    assert!(removed.job().is_none());
    assert!(removed.error().is_none());

    test_support::editor::core::connect(&mut core, &config);
    assert_ne!(old, core.host(&config.id).unwrap().instance());
    let mut replaced = pending_dialog(&core, &config);
    core.remove_host(&config.id).unwrap();
    test_support::editor::core::connect(&mut core, &config);
    assert!(replaced.drain(&core));
    assert!(replaced.job().is_none());
    assert!(replaced.error().is_none());
}

#[test]
fn inspector_scope_rejects_same_id_host_replacement_until_followed() {
    let directory = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(directory.path());
    let forge = ClosingListener::bind();
    let config = config(&forge);
    test_support::editor::core::connect(&mut core, &config);
    let mut conversation = editor_aqua::conversation::controller::Conversation::new(&core);
    conversation.open = Some((config.id.clone(), ThreadId("thread".into())));
    let mut inspector = editor_aqua::inspector::Inspector::default();
    assert!(inspector.follow(&core, &conversation));
    assert!(inspector.is_current(&core));
    core.remove_host(&config.id).unwrap();
    test_support::editor::core::connect(&mut core, &config);
    assert!(!inspector.is_current(&core));
    assert!(inspector.follow(&core, &conversation));
    assert!(inspector.is_current(&core));
    core.remove_host(&config.id).unwrap();
    assert!(!inspector.is_current(&core));
}

#[test]
fn a_fetch_that_finished_before_retry_does_not_cancel_the_retry() {
    let directory = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(directory.path());
    let forge = ClosingListener::bind();
    let config = config(&forge);
    test_support::editor::core::connect(&mut core, &config);
    let mut dialog = pending_dialog(&core, &config);
    dialog.fetch(&core);
    assert!(!dialog.drain(&core));
    assert!(
        dialog.loading(),
        "the earlier fetch's failure must not abort the retry"
    );
    assert!(dialog.error().is_none());
}
