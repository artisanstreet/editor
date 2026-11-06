use artisan_api::tls::{Identity, fingerprint};
use editor_aqua::core::{Core, EditorFiles, ForgeClient, HostConfig, HostId, HostKind, Screen};
use editor_aqua::shell::{
    host::ShellResources, selection::SelectionAllocator, workspace::Workspace,
};

fn host_config(fingerprint: &str) -> HostConfig {
    HostConfig {
        id: HostId("late-host".into()),
        name: "Late Forge".into(),
        address: "127.0.0.1:1".into(),
        fingerprint: fingerprint.to_owned(),
        kind: HostKind::Local,
    }
}

#[test]
fn start_waits_for_a_host_that_connects_after_the_workspace_mounts() {
    let dir = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: dir.path().to_owned(),
    })
    .unwrap();
    let mut ui = ShellResources::new(1.0).unwrap().ui;
    let (mut workspace, _) = Workspace::new(&core, None, SelectionAllocator::default().workspace());

    workspace.finish_events(&mut core, &mut ui);
    assert_eq!(core.screen(), &Screen::Home);

    let identity = Identity::generate("workspace-test").unwrap();
    let fingerprint = fingerprint(identity.certificate_der().unwrap().as_ref());
    let config = host_config(&fingerprint);
    let client = ForgeClient::new(&config.address, &fingerprint, &identity).unwrap();
    core.add_test_host(&config, client);
    workspace.finish_events(&mut core, &mut ui);
    assert_eq!(
        core.screen(),
        &Screen::NewThread {
            host: config.id.clone(),
            project: None
        }
    );
}
