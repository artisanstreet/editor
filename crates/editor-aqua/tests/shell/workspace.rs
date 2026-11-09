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

#[test]
fn removing_host_invalidates_its_workspace_draft_before_the_save_timer() {
    let directory = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: directory.path().to_owned(),
    })
    .unwrap();
    let identity = Identity::generate("workspace-draft-test").unwrap();
    let fingerprint = fingerprint(identity.certificate_der().unwrap().as_ref());
    let config = host_config(&fingerprint);
    let client = ForgeClient::new(&config.address, &fingerprint, &identity).unwrap();
    core.add_test_host(&config, client);
    core.navigate(Screen::NewThread {
        host: config.id.clone(),
        project: None,
    });
    let mut ui = ShellResources::new(1.0).unwrap().ui;
    let (mut workspace, _) = Workspace::new(&core, None, SelectionAllocator::default().workspace());
    workspace.sync_composer(&core, &mut ui);
    let draft = {
        let composer = workspace.composer.as_ref().unwrap().controller.borrow();
        composer.editor.borrow_mut().set_text("queued draft");
        composer.save();
        editor_aqua::composer::persistence::DraftHandle::clone(composer.session.as_ref().unwrap())
    };

    core.remove_host(&config.id).unwrap();
    workspace.handle_event(&core, &editor_aqua::core::CoreEvent::HostsChanged, false);

    assert!(draft.read(|state| state.removed()));
    assert_eq!(
        draft.read(|state| state.body.clone()),
        serde_json::Value::Null
    );
}

#[test]
fn sidebar_theme_changes_request_the_snapshot_sweep_in_both_sidebars() {
    for settings in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let mut core = Core::for_tests(EditorFiles {
            dir: directory.path().to_owned(),
        })
        .unwrap();
        let (mut workspace, _) =
            Workspace::new(&core, None, SelectionAllocator::default().workspace());
        let emit = workspace.sidebar_emit(settings);

        for dark in [false, true] {
            emit(editor_aqua::sidebar::view::ViewAction::SetDarkMode(dark));
            workspace.dispatch_actions(&mut core);

            assert_eq!(
                core.settings.get("dark_mode"),
                Some(&serde_json::Value::Bool(dark))
            );
            assert_eq!(workspace.take_theme_transition(), Some(dark));
            assert_eq!(workspace.take_theme_transition(), None);
        }
    }
}
