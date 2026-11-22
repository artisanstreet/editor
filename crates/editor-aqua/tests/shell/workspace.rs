use artisan_api::tls::{Identity, fingerprint};
use editor_aqua::core::{ForgeClient, HostConfig, HostId, HostKind, Screen};
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
    let mut core = test_support::editor::core::test_core(dir.path());
    let mut ui = ShellResources::new(1.0).unwrap().ui;
    let (mut workspace, _) = Workspace::new(&core, None, SelectionAllocator::default().workspace());

    workspace.finish_events(&mut core, &mut ui);
    assert_eq!(core.screen(), &Screen::Home);

    let identity = Identity::generate("workspace-test").unwrap();
    let fingerprint = fingerprint(identity.certificate_der().unwrap().as_ref());
    let config = host_config(&fingerprint);
    let client = ForgeClient::new(&config.address, &fingerprint, &identity).unwrap();
    test_support::editor::core::connect_client(&mut core, &config, client);
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
    let mut core = test_support::editor::core::test_core(directory.path());
    let identity = Identity::generate("workspace-draft-test").unwrap();
    let fingerprint = fingerprint(identity.certificate_der().unwrap().as_ref());
    let config = host_config(&fingerprint);
    let client = ForgeClient::new(&config.address, &fingerprint, &identity).unwrap();
    test_support::editor::core::connect_client(&mut core, &config, client);
    core.navigate(Screen::NewThread {
        host: config.id.clone(),
        project: None,
    });
    let mut ui = ShellResources::new(1.0).unwrap().ui;
    let (mut workspace, _) = Workspace::new(&core, None, SelectionAllocator::default().workspace());
    workspace.sync_composer(&core, &mut ui);
    let draft = {
        let mut composer = workspace
            .composer()
            .as_ref()
            .unwrap()
            .controller
            .borrow_mut();
        crate::recovered(composer.session.as_ref().unwrap());
        composer.sync(&core);
        composer.editor.borrow_mut().set_text("queued draft");
        composer.save();
        editor_aqua::composer::persistence::DraftHandle::clone(composer.session.as_ref().unwrap())
    };

    core.remove_host(&config.id).unwrap();
    workspace.handle_event(&core, &editor_aqua::core::CoreEvent::HostsChanged, false);

    crate::wait_for_draft(
        &draft,
        editor_aqua::composer::draft_session::DraftSession::removed,
    );
    assert_eq!(
        draft.read(|state| state.body().clone()),
        editor_aqua::composer::draft_body::DraftBody::default()
    );
}

#[test]
fn removing_host_keeps_the_composers_draft_and_unfinished_image_on_this_device() {
    use editor_aqua::composer::attachment::Source;
    use editor_aqua::composer::persistence::DraftSessions;
    use editor_aqua::composer::spool::DraftSpool;

    let directory = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(directory.path());
    let identity = Identity::generate("workspace-draft-test").unwrap();
    let fingerprint = fingerprint(identity.certificate_der().unwrap().as_ref());
    let config = host_config(&fingerprint);
    let client = ForgeClient::new(&config.address, &fingerprint, &identity).unwrap();
    test_support::editor::core::connect_client(&mut core, &config, client.clone());
    core.navigate(Screen::NewThread {
        host: config.id.clone(),
        project: None,
    });
    let mut ui = ShellResources::new(1.0).unwrap().ui;
    let (mut workspace, _) = Workspace::new(&core, None, SelectionAllocator::default().workspace());
    workspace.sync_composer(&core, &mut ui);
    let draft = {
        let mut composer = workspace
            .composer()
            .as_ref()
            .unwrap()
            .controller
            .borrow_mut();
        crate::recovered(composer.session.as_ref().unwrap());
        composer.sync(&core);
        composer
            .editor
            .borrow_mut()
            .set_text("typed while Forge was away");
        let mut image = std::io::Cursor::new(Vec::new());
        image::RgbaImage::new(1, 1)
            .write_to(&mut image, image::ImageFormat::Png)
            .unwrap();
        composer
            .attachments
            .as_mut()
            .unwrap()
            .add(Source::Bytes(image.into_inner()), "Pasted image".into());
        composer.edited();
        editor_aqua::composer::persistence::DraftHandle::clone(composer.session.as_ref().unwrap())
    };

    core.remove_host(&config.id).unwrap();
    workspace.handle_event(&core, &editor_aqua::core::CoreEvent::HostsChanged, false);
    crate::wait_for_draft(
        &draft,
        editor_aqua::composer::draft_session::DraftSession::removed,
    );

    let listed = |folder: &std::path::Path| -> Vec<std::path::PathBuf> {
        std::fs::read_dir(folder)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect()
    };
    let images = listed(&directory.path().join("drafts"))
        .iter()
        .flat_map(|scope| listed(scope))
        .flat_map(|kept| listed(&kept))
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "image")
        })
        .count();
    assert_eq!(
        images, 1,
        "the image still uploading is kept with the draft"
    );
    test_support::editor::core::connect_client(&mut core, &config, client.clone());
    let mut sessions = DraftSessions::with_spool(DraftSpool::new(
        directory.path(),
        editor_aqua::composer::spool::LocalDisk::shared(),
    ));
    let reopened = sessions
        .open(
            config.id.clone(),
            core.host(&config.id).unwrap().instance(),
            "new:".into(),
            client,
        )
        .unwrap();
    crate::recovered(&reopened);
    assert_eq!(
        reopened.read(|state| state.body().text.clone()),
        "typed while Forge was away"
    );
}

#[test]
fn sidebar_theme_changes_request_the_snapshot_sweep_in_both_sidebars() {
    for settings in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let mut core = test_support::editor::core::test_core(directory.path());
        let (mut workspace, _) =
            Workspace::new(&core, None, SelectionAllocator::default().workspace());
        let emit = workspace.sidebar_emit(settings);

        for dark in [false, true] {
            emit(editor_aqua::sidebar::view::ViewAction::SetDarkMode(dark));
            workspace.dispatch_actions(&mut core);

            assert_eq!(
                core.settings().get("dark_mode"),
                Some(&serde_json::Value::Bool(dark))
            );
            assert_eq!(workspace.take_theme_transition(), Some(dark));
            assert_eq!(workspace.take_theme_transition(), None);
        }
    }
}

#[test]
fn an_accepted_new_thread_only_takes_over_while_the_user_is_still_composing_it() {
    use artisan_api::ThreadId;
    use editor_aqua::composer::controller::ComposerTarget;
    use editor_aqua::core::SettingsPage;
    use editor_aqua::shell::workspace::Followed;

    let dir = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(dir.path());
    let identity = Identity::generate("accepted-send").unwrap();
    let fingerprint = fingerprint(identity.certificate_der().unwrap().as_ref());
    let config = host_config(&fingerprint);
    let client = ForgeClient::new(&config.address, &fingerprint, &identity).unwrap();
    test_support::editor::core::connect_client(&mut core, &config, client);
    let composing = Screen::NewThread {
        host: config.id.clone(),
        project: None,
    };
    let target = ComposerTarget::NewThread {
        host: config.id.clone(),
        project: None,
    };
    let sent = ThreadId::from("sent-thread");
    let opened = Screen::Thread {
        host: config.id.clone(),
        thread: sent.clone(),
    };

    core.navigate(composing.clone());
    assert_eq!(
        Workspace::follow_accepted(&mut core, &target, &sent),
        Followed::Navigated
    );
    assert_eq!(core.screen(), &opened);

    core.navigate(composing.clone());
    core.navigate(Screen::Settings(SettingsPage::General));
    assert_eq!(
        Workspace::follow_accepted(&mut core, &target, &sent),
        Followed::BehindSettings
    );
    assert_eq!(core.screen(), &Screen::Settings(SettingsPage::General));
    core.close_settings();
    assert_eq!(core.screen(), &opened);

    core.navigate(composing);
    core.navigate(Screen::Marketplace);
    assert_eq!(
        Workspace::follow_accepted(&mut core, &target, &sent),
        Followed::Stayed
    );
    assert_eq!(core.screen(), &Screen::Marketplace);
}
