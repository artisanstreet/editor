use artisan_api::ThreadId;
use editor_aqua::core::{Core, EditorFiles, ForgeClient, HostConfig, HostId, HostKind};
use editor_aqua::shell::resource_preview::{PreviewError, ResourcePreview};
use std::{
    sync::{Arc, mpsc},
    time::Duration,
};
use test_support::net::ClosingListener;

fn client(core: &Core, address: &str) -> ForgeClient {
    ForgeClient::new(address, &"0".repeat(64), core.identity.as_ref().unwrap()).unwrap()
}

fn config(address: &str) -> HostConfig {
    HostConfig {
        id: HostId("same-host".into()),
        name: "Forge".into(),
        address: address.into(),
        fingerprint: "0".repeat(64),
        kind: HostKind::Remote,
    }
}

fn pending_preview(core: &Core, config: &HostConfig) -> ResourcePreview {
    let host = core.host(&config.id).unwrap();
    ResourcePreview::new(
        "file.txt".into(),
        Some(1),
        Some((
            config.id.clone(),
            host.instance(),
            host.client.clone(),
            ThreadId("thread".into()),
        )),
    )
}

fn wait_for_result(preview: &ResourcePreview) {
    let (sender, receiver) = mpsc::channel();
    preview.wake_mailbox().unwrap().set_waker(Arc::new(move || {
        let _ = sender.send(());
    }));
    receiver.recv_timeout(Duration::from_secs(15)).unwrap();
}

#[test]
fn a_preview_whose_host_goes_away_says_so_instead_of_taking_the_late_answer() {
    let directory = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: directory.path().to_owned(),
    })
    .unwrap();
    let forge = ClosingListener::bind();
    let address = forge.address().to_string();
    let config = config(&address);
    core.add_test_host(&config, client(&core, &address));

    let mut current = pending_preview(&core, &config);
    wait_for_result(&current);
    assert!(current.drain(&core));
    assert!(matches!(current.content, Some(Err(_))));

    let mut removed = pending_preview(&core, &config);
    wait_for_result(&removed);
    let old = core.host(&config.id).unwrap().instance();
    core.remove_host(&config.id).unwrap();
    assert!(removed.drain(&core));
    assert!(matches!(
        removed.content,
        Some(Err(PreviewError::HostGone { .. }))
    ));

    core.add_test_host(&config, client(&core, &address));
    let new = core.host(&config.id).unwrap().instance();
    assert_ne!(old, new);
    let mut replaced = pending_preview(&core, &config);
    wait_for_result(&replaced);
    core.add_test_host(&config, client(&core, &address));
    assert!(replaced.drain(&core));
    assert!(matches!(
        replaced.content,
        Some(Err(PreviewError::HostGone { .. }))
    ));
}
