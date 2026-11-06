use artisan_api::{ItemId, ThreadId};
use editor_aqua::{
    core::{Core, EditorFiles, ForgeClient, HostConfig, HostId, HostKind},
    inspector::TerminalDialog,
};
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

fn pending_dialog(core: &Core, config: &HostConfig) -> TerminalDialog {
    let mut dialog = TerminalDialog::new(core);
    let open = (config.id.clone(), ThreadId("thread".into()));
    assert!(dialog.open(core, Some(&open), &ItemId("item".into())));
    let deadline = Instant::now() + Duration::from_secs(15);
    while !dialog.fetch_finished() {
        assert!(Instant::now() < deadline, "terminal fetch did not finish");
        std::thread::sleep(Duration::from_millis(5));
    }
    dialog
}

#[test]
fn terminal_fetch_completion_is_discarded_after_removal_and_replacement() {
    let directory = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: directory.path().to_owned(),
    })
    .unwrap();
    let config = config();
    core.add_test_host(&config, client(&core));
    let mut current = pending_dialog(&core, &config);
    assert!(current.drain(&core));
    assert!(
        current
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("Couldn't load shell output:"))
    );

    let mut removed = pending_dialog(&core, &config);
    let old = core.host(&config.id).unwrap().instance();
    core.remove_host(&config.id).unwrap();
    assert!(removed.drain(&core));
    assert!(removed.job.is_none());
    assert!(removed.error.is_none());

    core.add_test_host(&config, client(&core));
    assert_ne!(old, core.host(&config.id).unwrap().instance());
    let mut replaced = pending_dialog(&core, &config);
    core.add_test_host(&config, client(&core));
    assert!(replaced.drain(&core));
    assert!(replaced.job.is_none());
    assert!(replaced.error.is_none());
}

#[test]
fn inspector_scope_rejects_same_id_host_replacement_until_followed() {
    let directory = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: directory.path().to_owned(),
    })
    .unwrap();
    let config = config();
    core.add_test_host(&config, client(&core));
    let mut conversation = editor_aqua::conversation::controller::Conversation::new(&core);
    conversation.open = Some((config.id.clone(), ThreadId("thread".into())));
    let mut inspector = editor_aqua::inspector::Inspector::default();
    assert!(inspector.follow(&core, &conversation));
    assert!(inspector.is_current(&core));
    core.add_test_host(&config, client(&core));
    assert!(!inspector.is_current(&core));
    assert!(inspector.follow(&core, &conversation));
    assert!(inspector.is_current(&core));
    core.remove_host(&config.id).unwrap();
    assert!(!inspector.is_current(&core));
}
