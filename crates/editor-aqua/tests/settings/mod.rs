use editor_aqua::settings::{extensions, prompt};

use artisan_api::{GlobalPrompt, McpServer};
use extensions::{ExtensionsForm, ServerFields};
use prompt::PromptForm;

#[test]
fn mcp_validation_and_legacy_transport() {
    let fields = ServerFields {
        name: " server ".into(),
        url: " https://example.com/mcp ".into(),
        headers: "{\"Authorization\":\"token\"}".into(),
        ..ServerFields::default()
    };
    let previous = McpServer {
        sse: true,
        ..McpServer::default()
    };
    let server = fields.parse(Some(&previous)).unwrap();
    assert_eq!(server.name, "server");
    assert_eq!(server.url.as_deref(), Some("https://example.com/mcp"));
    assert!(server.sse);
    let mut invalid = fields;
    invalid.args = "[\"arg\"]".into();
    assert_eq!(
        invalid.parse(None).unwrap_err(),
        "HTTP servers do not take process arguments or environment variables."
    );
}

#[test]
fn mcp_program_is_trimmed_and_blank_programs_are_refused() {
    let fields = ServerFields {
        name: "local".into(),
        command: "  /opt/My Tools/server  ".into(),
        ..ServerFields::default()
    };
    assert_eq!(fields.parse(None).unwrap().command, "/opt/My Tools/server");
    let blank = ServerFields {
        name: "local".into(),
        command: "   ".into(),
        ..ServerFields::default()
    };
    assert_eq!(
        blank.parse(None).unwrap_err(),
        "Specify either a program or an HTTP URL."
    );
}

fn host_target(
    core: &editor_aqua::core::Core,
    id: &editor_aqua::core::HostId,
) -> Option<(String, editor_aqua::core::HostInstance)> {
    core.host(id).map(|host| (id.0.clone(), host.instance()))
}

#[test]
fn mcp_completion_cannot_close_reopened_form() {
    let dir = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(dir.path());
    let host = add_host(&mut core, "host");
    let mut form = ExtensionsForm::default();
    form.sync_host(host_target(&core, &host));
    form.open(None);
    form.fields.name = "first".into();
    form.fields.command = "server".into();
    let request = form.save().unwrap();
    form.open(None);
    form.fields.name = "second".into();
    form.complete(&request, Ok(()));
    assert!(form.editing.is_some());
    assert_eq!(form.fields.name, "second");
}

#[test]
fn switching_hosts_mid_save_keeps_the_mcp_form_and_its_outcome_with_its_host() {
    let dir = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(dir.path());
    let first = add_host(&mut core, "first-forge");
    let second = add_host(&mut core, "second-forge");
    let mut form = ExtensionsForm::default();
    let save_on_first = |form: &mut ExtensionsForm| {
        form.sync_host(host_target(&core, &first));
        form.open(None);
        form.fields.name = "github".into();
        form.fields.command = "github-mcp".into();
        let request = form.save().unwrap();
        form.sync_host(host_target(&core, &second));
        assert!(form.editing.is_none() && !form.saving);
        request
    };

    let failing = save_on_first(&mut form);
    form.sync_host(host_target(&core, &first));
    assert!(form.saving, "the save on the first Forge is still running");
    assert_eq!(form.fields.name, "github");
    form.sync_host(host_target(&core, &second));
    form.complete(
        &failing,
        Err(editor_aqua::core::Problem::new("Forge refused")),
    );
    assert!(
        form.editing.is_none(),
        "the second Forge's form stays closed"
    );
    form.sync_host(host_target(&core, &first));
    assert!(!form.saving);
    assert!(
        form.editing.is_some(),
        "a failed save leaves its form open to try again"
    );
    assert_eq!(form.fields.command, "github-mcp");

    let saved = save_on_first(&mut form);
    form.complete(&saved, Ok(()));
    form.sync_host(host_target(&core, &first));
    assert!(form.editing.is_none() && !form.saving);
}

#[test]
fn switching_hosts_keeps_a_half_typed_mcp_server() {
    use editor_aqua::core::{Screen, SettingsPage};
    use editor_aqua::settings::controller::SettingsController;
    let dir = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(dir.path());
    let first = add_host(&mut core, "first-forge");
    let second = add_host(&mut core, "second-forge");
    let mut settings = SettingsController::default();
    let open = |core: &mut editor_aqua::core::Core,
                settings: &mut SettingsController,
                host: &editor_aqua::core::HostId| {
        core.navigate(Screen::NewThread {
            host: host.clone(),
            project: None,
        });
        settings.sync(core);
        core.navigate(Screen::Settings(SettingsPage::Extensions));
        settings.sync(core);
    };
    open(&mut core, &mut settings, &first);
    settings.open_mcp(None);
    settings.mcp_editors[0].borrow_mut().set_text("github");
    open(&mut core, &mut settings, &second);
    assert!(settings.extensions.editing.is_none());
    assert_eq!(settings.mcp_editors[0].borrow().text(), "");
    open(&mut core, &mut settings, &first);
    assert!(settings.extensions.editing.is_some());
    assert_eq!(settings.mcp_editors[0].borrow().text(), "github");
}

#[test]
fn prompt_save_preserves_newer_draft_and_rejects_old_host() {
    let mut form = PromptForm::default();
    assert!(form.sync_host(Some("a".into())));
    let initial = GlobalPrompt {
        version: 1,
        text: "old".into(),
        ..GlobalPrompt::default()
    };
    assert_eq!(form.load(&initial, "", false).as_deref(), Some("old"));
    form.changed();
    let request = form.save("first").unwrap();
    form.changed();
    let saved = GlobalPrompt {
        version: 2,
        text: "first".into(),
        ..GlobalPrompt::default()
    };
    assert_eq!(
        form.complete(&request, Ok((2, Some(saved))), "first second"),
        None
    );
    assert_eq!(form.saved, "first");
    assert_eq!(form.status("first second"), "Unsaved changes");
    let request = form.save("first second").unwrap();
    form.sync_host(Some("b".into()));
    form.error = Some(editor_aqua::core::Problem::new("host b"));
    form.complete(
        &request,
        Err(editor_aqua::core::Problem::new("old failure")),
        "",
    );
    assert_eq!(
        form.error
            .as_ref()
            .map(editor_aqua::core::Problem::headline),
        Some("host b")
    );
}

#[test]
fn second_factor_distinguishes_six_ascii_digits() {
    use editor_aqua::core::account::SecondFactor;
    assert_eq!(
        SecondFactor::parse(" 123456 "),
        Some(SecondFactor::Authenticator("123456".into()))
    );
    assert_eq!(
        SecondFactor::parse("１２３４５６"),
        Some(SecondFactor::Recovery("１２３４５６".into()))
    );
    assert_eq!(SecondFactor::parse("  "), None);
}

#[test]
fn delayed_setting_failure_does_not_rollback_replaced_host() {
    use axum::http::StatusCode;
    use editor_aqua::settings::controller::SettingsController;
    use serde_json::json;

    let dir = tempfile::tempdir().unwrap();
    let forge = stub_forge();
    let config = test_support::editor::core::stub_host("same-host", "Test Forge", &forge);
    let mut core = test_support::editor::core::test_core(dir.path());
    connect_live(&mut core, &config);
    let original = core.host(&config.id).unwrap().instance();
    let host = core.host_mut(&config.id).unwrap();
    host.settings.insert("suggestions", json!(false));
    host.settings.insert("summary_titles", json!(false));
    let mut settings = SettingsController::default();
    for key in ["suggestions", "summary_titles"] {
        settings.set_host_setting_for(&mut core, config.id.clone(), original, key, json!(true));
    }
    let request = forge.take("PUT", "/settings/suggestions");
    let removed_request = forge.take("PUT", "/settings/summary_titles");
    core.remove_host(&config.id).unwrap();
    assert!(core.host_if_instance(&config.id, original).is_none());
    removed_request.fail(StatusCode::SERVICE_UNAVAILABLE, "internal", "late failure");
    drain_settings(&mut settings, &mut core, "the removed host's late failure");
    assert!(core.host(&config.id).is_none());
    connect_live(&mut core, &config);
    let host = core.host_mut(&config.id).unwrap();
    assert_ne!(host.instance(), original);
    host.settings.insert("suggestions", json!("replacement"));
    settings.set_host_setting_for(
        &mut core,
        config.id.clone(),
        original,
        "suggestions",
        json!(true),
    );
    assert_eq!(
        core.host(&config.id).unwrap().setting("suggestions"),
        Some(&json!("replacement"))
    );
    request.fail(StatusCode::SERVICE_UNAVAILABLE, "internal", "late failure");
    drain_settings(&mut settings, &mut core, "the replaced host's late failure");
    assert_eq!(
        core.host(&config.id).unwrap().setting("suggestions"),
        Some(&json!("replacement"))
    );
}

fn stub_forge() -> test_support::editor::forge::StubForge {
    editor_aqua::core::runtime::runtime().block_on(test_support::editor::forge::StubForge::serve(
        test_support::editor::forge::empty_state(),
        axum::Router::new(),
    ))
}

fn connect_live(core: &mut editor_aqua::core::Core, config: &editor_aqua::core::HostConfig) {
    test_support::editor::core::connect(core, config);
    test_support::wait::wait_until(
        "the stub Forge to go live",
        std::time::Duration::from_secs(10),
        || {
            core.drain_with(|_, _| {});
            core.host(&config.id)
                .is_some_and(editor_aqua::core::HostData::is_live)
        },
    );
}

fn drain_settings(
    settings: &mut editor_aqua::settings::controller::SettingsController,
    core: &mut editor_aqua::core::Core,
    what: &str,
) {
    test_support::wait::wait_until(what, std::time::Duration::from_secs(10), || {
        settings.drain(core)
    });
}

#[test]
fn engine_login_code_requires_text_and_clears_only_on_submission() {
    use artisan_api::EngineId;
    use editor_aqua::settings::engines::EnginesForm;
    let mut form = EnginesForm::default();
    form.sync_host(Some("forge".into()));
    form.prepare(EngineId::Claude);
    let editor = std::rc::Rc::clone(form.login_codes.get(&EngineId::Claude).unwrap());
    editor.borrow_mut().set_text("   ");
    assert!(form.login_code(EngineId::Claude).is_none());
    assert_eq!(editor.borrow().text(), "   ");
    assert_eq!(
        form.errors
            .get(&EngineId::Claude)
            .map(editor_aqua::core::Problem::headline),
        Some("Paste the code the sign-in page showed you first.")
    );
    editor.borrow_mut().set_text("  code-value  ");
    assert_eq!(
        form.login_code(EngineId::Claude).as_deref(),
        Some("code-value")
    );
    assert_eq!(editor.borrow().text(), "");
}

#[test]
fn dispatch_reports_visible_changes_without_blocking_side_effects() {
    use editor_aqua::settings::{
        controller::SettingsController,
        view::{ViewAction, dispatch},
    };
    use serde_json::json;
    let dir = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(dir.path());
    let mut settings = SettingsController::default();
    assert!(!dispatch(&mut settings, &mut core, ViewAction::McpCancel));
    assert!(!dispatch(
        &mut settings,
        &mut core,
        ViewAction::OpenUrl("https://example.com".into())
    ));
    assert!(!dispatch(
        &mut settings,
        &mut core,
        ViewAction::SetNotifications(true)
    ));
    assert_eq!(core.settings().get("notifications"), Some(&json!(true)));
    assert!(dispatch(
        &mut settings,
        &mut core,
        ViewAction::SetNotifications(false)
    ));
    assert!(!dispatch(
        &mut settings,
        &mut core,
        ViewAction::SetNotifications(false)
    ));
    assert!(dispatch(
        &mut settings,
        &mut core,
        ViewAction::FontMenu(true)
    ));
    assert!(!dispatch(
        &mut settings,
        &mut core,
        ViewAction::FontMenu(true)
    ));
    assert!(dispatch(
        &mut settings,
        &mut core,
        ViewAction::FontMenu(false)
    ));
    settings.prompt.loaded = true;
    settings.prompt_editor.borrow_mut().set_text("first");
    assert!(dispatch(
        &mut settings,
        &mut core,
        ViewAction::PromptChanged
    ));
    settings.prompt_editor.borrow_mut().set_text("first second");
    assert!(!dispatch(
        &mut settings,
        &mut core,
        ViewAction::PromptChanged
    ));
    assert!(dispatch(
        &mut settings,
        &mut core,
        ViewAction::PromptPresentationChanged
    ));
}

#[test]
fn every_settings_page_shows_content_without_a_host() {
    use artisan_api::EngineId;
    use editor_aqua::core::{Screen, SettingsPage, SettingsSection};
    use editor_aqua::settings::{about::BuildInfo, controller::SettingsController, view::build};
    use editor_aqua::ui::theme::ArtisanTheme;
    use std::rc::Rc;
    let dir = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(dir.path());
    let controller = SettingsController::default();
    let mut pages = vec![
        SettingsPage::General,
        SettingsPage::Hosts,
        SettingsPage::About,
        SettingsPage::Threads,
        SettingsPage::GlobalPrompt,
        SettingsPage::Extensions,
        SettingsPage::Account,
    ];
    for engine in EngineId::ALL {
        pages.push(SettingsPage::Engine(engine));
        pages.extend(
            SettingsPage::Engine(engine)
                .sections()
                .into_iter()
                .map(SettingsPage::Section),
        );
    }
    for parent in [
        SettingsPage::General,
        SettingsPage::Threads,
        SettingsPage::Extensions,
    ] {
        pages.extend(parent.sections().into_iter().map(SettingsPage::Section));
    }
    assert!(pages.contains(&SettingsPage::Section(SettingsSection::Appearance)));
    for page in pages {
        core.navigate(Screen::Settings(page));
        assert_eq!(core.screen(), &Screen::Settings(page));
        let texts = page_texts(build(
            &controller,
            &core,
            &ArtisanTheme::resolved(),
            &BuildInfo::default(),
            aqua_core::Size::new(900.0, 700.0),
            true,
            Rc::new(|_| {}),
        ));
        assert!(
            texts.iter().any(|text| !text.trim().is_empty()),
            "{page:?} renders an empty body"
        );
    }
}

fn page_texts(element: aqua_ui::element::Element) -> Vec<String> {
    use aqua::UiApp;
    use aqua_text::{SharedFonts, SharedRuns};
    use aqua_ui::tree::NodeKind;
    let mut app = UiApp::new(SharedFonts::new(), SharedRuns::new());
    let mut element = Some(element);
    app.mount(move |_| element.take().expect("the element mounts once"))
        .expect("the test view mounts");
    app.set_viewport(900.0, 700.0, 1.0);
    app.build_frame().expect("the test frame builds");
    let tree = app.tree();
    let mut pending = vec![tree.root().expect("the mounted tree has a root")];
    let mut texts = Vec::new();
    while let Some(id) = pending.pop() {
        let Some(node) = tree.get(id) else { continue };
        if let NodeKind::Text(content) = &node.kind {
            texts.push(content.text().to_owned());
        }
        pending.extend(node.children.iter().copied());
    }
    texts
}

#[test]
fn host_bound_pages_show_their_content_once_a_host_is_connected() {
    use artisan_api::EngineId;
    use artisan_api::tls::{Identity, fingerprint};
    use editor_aqua::core::{
        Core, ForgeClient, HostConfig, HostId, HostKind, Screen, SettingsPage,
    };
    use editor_aqua::settings::{about::BuildInfo, controller::SettingsController, view::build};
    use editor_aqua::ui::theme::ArtisanTheme;
    use std::rc::Rc;

    let dir = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(dir.path());
    let controller = SettingsController::default();
    let identity = Identity::generate("settings-with-host").unwrap();
    let fingerprint = fingerprint(identity.certificate_der().unwrap().as_ref());
    let config = HostConfig {
        id: HostId("with-host".into()),
        name: "Desk Forge".into(),
        address: "127.0.0.1:1".into(),
        fingerprint: fingerprint.clone(),
        kind: HostKind::Local,
    };
    let client = ForgeClient::new(&config.address, &fingerprint, &identity).unwrap();
    let render = |core: &mut Core, page: SettingsPage| {
        core.navigate(Screen::Settings(page));
        page_texts(build(
            &controller,
            core,
            &ArtisanTheme::resolved(),
            &BuildInfo::default(),
            aqua_core::Size::new(900.0, 700.0),
            true,
            Rc::new(|_| {}),
        ))
    };
    let host_pages = [
        SettingsPage::Threads,
        SettingsPage::Extensions,
        SettingsPage::Engine(EngineId::ALL[0]),
    ];
    for page in host_pages {
        let texts = render(&mut core, page);
        assert!(
            texts
                .iter()
                .any(|text| text.contains("Pair a Forge on the Account page")),
            "{page:?} without a host"
        );
    }
    test_support::editor::core::connect_client(&mut core, &config, client);
    for page in host_pages {
        let texts = render(&mut core, page);
        assert!(
            texts.iter().any(|text| text == page.label()),
            "{page:?} header"
        );
        assert!(
            !texts
                .iter()
                .any(|text| text.contains("Pair a Forge on the Account page")),
            "{page:?} with a host"
        );
    }
    let hosts = render(&mut core, SettingsPage::Hosts);
    assert!(hosts.iter().any(|text| text == "Desk Forge"));
}

fn logical_rects(element: aqua_ui::element::Element, scale: f32) -> Vec<(String, [f32; 4])> {
    use aqua::UiApp;
    use aqua_text::{SharedFonts, SharedRuns};
    use aqua_ui::tree::NodeKind;
    let mut app = UiApp::new(SharedFonts::new(), SharedRuns::new());
    let mut element = Some(element);
    app.mount(move |_| element.take().expect("the element mounts once"))
        .expect("the test view mounts");
    app.tree_mut().set_gpui_layout_scale(Some(scale));
    app.set_viewport(900.0, 700.0, scale);
    app.build_frame().expect("the test frame builds");
    let tree = app.tree();
    let mut pending = vec![tree.root().expect("the mounted tree has a root")];
    let mut rects = Vec::new();
    while let Some(id) = pending.pop() {
        let Some(node) = tree.get(id) else { continue };
        if let (NodeKind::Text(content), Some(rect)) = (&node.kind, tree.rect(id)) {
            rects.push((content.text().to_owned(), [rect.x, rect.y, rect.w, rect.h]));
        }
        pending.extend(node.children.iter().copied());
    }
    rects.sort_by(|a, b| a.0.cmp(&b.0).then(a.1[1].total_cmp(&b.1[1])));
    rects
}

#[test]
fn settings_pages_keep_their_layout_at_fractional_and_double_scale() {
    use editor_aqua::core::{Screen, SettingsPage};
    use editor_aqua::settings::{about::BuildInfo, controller::SettingsController, view::build};
    use editor_aqua::ui::theme::ArtisanTheme;
    use std::rc::Rc;

    let dir = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(dir.path());
    let controller = SettingsController::default();
    for page in [
        SettingsPage::General,
        SettingsPage::Hosts,
        SettingsPage::About,
        SettingsPage::Account,
    ] {
        core.navigate(Screen::Settings(page));
        let at = |scale: f32| {
            logical_rects(
                build(
                    &controller,
                    &core,
                    &ArtisanTheme::resolved(),
                    &BuildInfo::default(),
                    aqua_core::Size::new(900.0, 700.0),
                    true,
                    Rc::new(|_| {}),
                ),
                scale,
            )
        };
        let unit = at(1.0);
        assert!(!unit.is_empty(), "{page:?} has no text");
        for (scale, position, size) in [(1.5, 6.0, 2.0), (2.0, 0.5, 0.5)] {
            let scaled = at(scale);
            assert_eq!(unit.len(), scaled.len(), "{page:?} text count at {scale}");
            for (expected, actual) in unit.iter().zip(&scaled) {
                assert_eq!(expected.0, actual.0);
                for (axis, (left, right)) in expected.1.iter().zip(actual.1).enumerate() {
                    let rounding = if axis < 2 { position } else { size };
                    assert!(
                        (left - right).abs() <= rounding,
                        "{page:?} {:?} axis {axis}: {left} at 1x, {right} at {scale}x",
                        expected.0,
                    );
                }
            }
        }
    }
}

fn add_host(core: &mut editor_aqua::core::Core, id: &str) -> editor_aqua::core::HostId {
    use artisan_api::tls::{Identity, fingerprint};
    use editor_aqua::core::{ForgeClient, HostConfig, HostId, HostKind};
    let identity = Identity::generate("settings-host").expect("a test identity generates");
    let fingerprint = fingerprint(
        identity
            .certificate_der()
            .expect("the test identity encodes its certificate")
            .as_ref(),
    );
    let config = HostConfig {
        id: HostId(id.into()),
        name: id.into(),
        address: "127.0.0.1:1".into(),
        fingerprint: fingerprint.clone(),
        kind: HostKind::Local,
    };
    let client = ForgeClient::new(&config.address, &fingerprint, &identity)
        .expect("the test Forge client builds");
    test_support::editor::core::connect_client(core, &config, client);
    config.id
}

fn retention_target(
    core: &mut editor_aqua::core::Core,
) -> editor_aqua::settings::preferences::SettingKey {
    let id = add_host(core, "writes-host");
    editor_aqua::settings::preferences::SettingKey {
        instance: core
            .host(&id)
            .expect("the test host is configured")
            .instance(),
        host: id,
        key: artisan_api::THREAD_RETENTION_DAYS_KEY,
    }
}

fn offline(
    target: &editor_aqua::settings::preferences::SettingKey,
) -> Result<(), editor_aqua::settings::preferences::SaveSettingError> {
    Err(editor_aqua::settings::preferences::SaveSettingError::Send {
        host: target.host.clone(),
        key: target.key,
        source: test_support::editor::forge::inconsistent(
            &format!("/settings/{}", target.key),
            "offline",
        ),
    })
}

#[test]
fn an_older_failed_setting_never_rolls_back_a_newer_choice() {
    use editor_aqua::settings::preferences::SettingWrites;
    use serde_json::json;
    let dir = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(dir.path());
    let target = retention_target(&mut core);
    let key = target.key;
    let mut settings = editor_aqua::core::HostSettings::new(target.host.clone());
    settings.insert(key, json!(90));
    let mut writes = SettingWrites::default();

    let seven = writes
        .begin(&mut settings, target.clone(), json!(7))
        .expect("the first choice is sent");
    assert!(
        writes
            .begin(&mut settings, target.clone(), json!(14))
            .is_none(),
        "a second choice waits for the first to finish"
    );
    let fourteen = writes
        .complete(Some(&mut settings), &seven, offline(&target))
        .expect("the newer choice is sent after the older one finishes");
    assert_eq!(settings.get(key), Some(&json!(14)));
    assert!(
        writes
            .complete(Some(&mut settings), &fourteen, Ok(()))
            .is_none()
    );
    assert_eq!(settings.get(key), Some(&json!(14)));

    let thirty = writes
        .begin(&mut settings, target.clone(), json!(30))
        .expect("sent");
    assert!(
        writes
            .complete(Some(&mut settings), &thirty, offline(&target))
            .is_none()
    );
    assert_eq!(
        settings.get(key),
        Some(&json!(14)),
        "a failure restores the last value Forge accepted"
    );

    let sixty = writes
        .begin(&mut settings, target.clone(), json!(60))
        .expect("sent");
    settings.insert(key, json!(45));
    assert!(
        writes
            .complete(Some(&mut settings), &sixty, offline(&target))
            .is_none()
    );
    assert_eq!(
        settings.get(key),
        Some(&json!(45)),
        "a failure never overwrites a value Forge reported since"
    );
}

#[test]
fn switching_hosts_keeps_unsaved_global_instructions() {
    use editor_aqua::core::{Core, Screen, SettingsPage};
    use editor_aqua::settings::controller::SettingsController;
    use std::sync::Arc;
    let dir = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(dir.path());
    let first = add_host(&mut core, "first-forge");
    let second = add_host(&mut core, "second-forge");
    for (id, text) in [(&first, "first saved"), (&second, "second saved")] {
        core.host_mut(id).unwrap().global_prompt = Some(Arc::new(GlobalPrompt {
            version: 3,
            text: text.into(),
            candidates: Vec::new(),
            engines: Vec::new(),
        }));
    }
    let mut settings = SettingsController::default();
    let open_prompt =
        |core: &mut Core, settings: &mut SettingsController, host: &editor_aqua::core::HostId| {
            core.navigate(Screen::NewThread {
                host: host.clone(),
                project: None,
            });
            settings.sync(core);
            core.navigate(Screen::Settings(SettingsPage::GlobalPrompt));
            settings.sync(core);
        };
    open_prompt(&mut core, &mut settings, &first);
    assert_eq!(settings.prompt_editor.borrow().text(), "first saved");
    settings
        .prompt_editor
        .borrow_mut()
        .set_text("first, edited but not saved");
    settings.prompt.changed();

    open_prompt(&mut core, &mut settings, &second);
    assert_eq!(settings.prompt_editor.borrow().text(), "second saved");

    open_prompt(&mut core, &mut settings, &first);
    assert_eq!(
        settings.prompt_editor.borrow().text(),
        "first, edited but not saved"
    );
    assert_eq!(
        settings
            .prompt
            .status(settings.prompt_editor.borrow().text()),
        "Unsaved changes"
    );
}

#[test]
fn an_mcp_form_that_is_saving_keeps_its_fields_and_cannot_be_dismissed() {
    use editor_aqua::core::{Screen, SettingsPage};
    use editor_aqua::settings::controller::SettingsController;
    let dir = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(dir.path());
    add_host(&mut core, "mcp-forge");
    core.navigate(Screen::Settings(SettingsPage::Extensions));
    let mut settings = SettingsController::default();
    settings.sync(&mut core);
    settings.open_mcp(None);
    settings.mcp_editors[0].borrow_mut().set_text("github");
    settings.mcp_editors[1].borrow_mut().set_text("github-mcp");
    settings.save_mcp(&core);
    assert!(settings.extensions.saving);
    assert!(
        settings
            .mcp_editors
            .iter()
            .all(|editor| editor.borrow().is_disabled()),
        "typing into a form that is being saved would be thrown away"
    );
    settings.cancel_mcp();
    assert!(
        settings.extensions.editing.is_some(),
        "Cancel cannot hide a save that keeps running"
    );
}

#[test]
fn about_page_opens_the_third_party_notices() {
    use aqua::UiApp;
    use aqua_core::{InputEvent, Point, PointerButton, PointerEvent, PointerKind};
    use aqua_text::{SharedFonts, SharedRuns};
    use editor_aqua::core::{Screen, SettingsPage};
    use editor_aqua::settings::{
        about::BuildInfo,
        controller::SettingsController,
        view::{ViewAction, build},
    };
    use editor_aqua::ui::theme::ArtisanTheme;
    use std::{cell::RefCell, rc::Rc};
    let dir = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(dir.path());
    core.navigate(Screen::Settings(SettingsPage::About));
    let controller = SettingsController::default();
    let emitted = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&emitted);
    let page = build(
        &controller,
        &core,
        &ArtisanTheme::resolved(),
        &BuildInfo::default(),
        aqua_core::Size::new(900.0, 700.0),
        true,
        Rc::new(move |action| sink.borrow_mut().push(action)),
    );
    let mut app = UiApp::new(SharedFonts::new(), SharedRuns::new());
    let mut page = Some(page);
    app.mount(move |_| page.take().unwrap()).unwrap();
    app.set_viewport(900.0, 700.0, 1.0);
    app.build_frame().unwrap();
    let key = aqua_ui::tree::hash_key(aqua_ui::element::Key::from("about-open-notices"));
    let button = app
        .tree()
        .traverse()
        .into_iter()
        .find(|id| {
            app.tree()
                .get(*id)
                .is_some_and(|node| node.key == Some(key))
        })
        .expect("the About page offers the third-party notices");
    let bounds = app.tree().rect(button).unwrap();
    let pointer = PointerEvent {
        contact: 0,
        kind: PointerKind::Mouse,
        position: Point::new(bounds.x + bounds.w / 2.0, bounds.y + bounds.h / 2.0),
        buttons: 1,
    };
    app.handle_input(&InputEvent::PointerDown(pointer, PointerButton::Primary));
    app.handle_input(&InputEvent::PointerUp(pointer, PointerButton::Primary));
    assert!(matches!(
        emitted.borrow().as_slice(),
        [ViewAction::OpenNotices]
    ));
}

fn silent_host(
    core: &mut editor_aqua::core::Core,
    id: &str,
) -> (std::net::TcpListener, editor_aqua::core::HostId) {
    use artisan_api::tls::{Identity, fingerprint};
    use editor_aqua::core::{ForgeClient, HostConfig, HostId, HostKind};
    let silent = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("the test listener binds");
    let identity = Identity::generate("settings-silent-host").expect("a test identity generates");
    let fingerprint = fingerprint(
        identity
            .certificate_der()
            .expect("the test identity encodes its certificate")
            .as_ref(),
    );
    let config = HostConfig {
        id: HostId(id.into()),
        name: id.into(),
        address: silent
            .local_addr()
            .expect("the test listener has an address")
            .to_string(),
        fingerprint: fingerprint.clone(),
        kind: HostKind::Local,
    };
    let client = ForgeClient::new(&config.address, &fingerprint, &identity)
        .expect("the test Forge client builds");
    test_support::editor::core::connect_client(core, &config, client);
    (silent, config.id)
}

fn render_settings(
    controller: &editor_aqua::settings::controller::SettingsController,
    core: &editor_aqua::core::Core,
    emit: std::rc::Rc<dyn Fn(editor_aqua::settings::view::ViewAction)>,
) -> aqua::UiApp {
    use aqua_text::{SharedFonts, SharedRuns};
    use editor_aqua::settings::{about::BuildInfo, view::build};
    use editor_aqua::ui::theme::ArtisanTheme;
    let view = build(
        controller,
        core,
        &ArtisanTheme::resolved(),
        &BuildInfo::default(),
        aqua_core::Size::new(900.0, 2400.0),
        true,
        emit,
    );
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| view).expect("the test view mounts");
    app.set_viewport(900.0, 2400.0, 1.0);
    app.build_frame().expect("the test frame builds");
    app
}

#[test]
fn a_failed_forge_setting_names_the_setting_and_its_cause_and_tries_again() {
    use axum::http::StatusCode;
    use editor_aqua::core::{Screen, SettingsPage};
    use editor_aqua::settings::{
        controller::SettingsController,
        preferences::SettingKey,
        view::{ViewAction, dispatch},
    };
    use serde_json::json;
    use std::{cell::RefCell, rc::Rc};
    let dir = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(dir.path());
    let forge = stub_forge();
    let config = test_support::editor::core::stub_host("Desk", "Desk", &forge);
    connect_live(&mut core, &config);
    let id = config.id.clone();
    core.navigate(Screen::Settings(SettingsPage::Threads));
    let mut settings = SettingsController::default();
    settings.sync(&mut core);
    let target = SettingKey {
        instance: core.host(&id).unwrap().instance(),
        host: id.clone(),
        key: "suggestions",
    };
    assert!(dispatch(
        &mut settings,
        &mut core,
        ViewAction::SetHostSetting(id.clone(), target.instance, "suggestions", false)
    ));
    let sent = forge.take("PUT", "/settings/suggestions");
    assert_eq!(sent.json::<serde_json::Value>(), json!(false));
    sent.fail(
        StatusCode::SERVICE_UNAVAILABLE,
        "internal",
        "the host did not answer",
    );
    drain_settings(&mut settings, &mut core, "the failed setting");
    assert_ne!(
        core.host(&id).unwrap().setting("suggestions"),
        Some(&json!(false))
    );
    let actions = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&actions);
    let mut app = render_settings(
        &settings,
        &core,
        Rc::new(move |action| sink.borrow_mut().push(action)),
    );
    let texts = test_support::editor::ui::painted_texts(&app);
    assert!(
        texts.iter().any(|text| text
            == "Couldn't save “Suggest the next message” on Desk. Forge ran into a problem."),
        "{texts:?}"
    );
    assert!(
        !texts
            .iter()
            .any(|text| text.contains("/settings/suggestions") || text.contains("503")),
        "the request and status stay in the details, off the page: {texts:?}"
    );
    test_support::editor::ui::click(&mut app, "settings-retry-suggestions");
    let retry = actions.borrow_mut().pop().unwrap();
    assert!(matches!(&retry, ViewAction::RetryHostSetting(key) if *key == target));
    assert!(dispatch(&mut settings, &mut core, retry));
    assert_eq!(
        core.host(&id).unwrap().setting("suggestions"),
        Some(&json!(false)),
        "Try again sends the choice that failed"
    );
    assert_eq!(
        forge
            .take("PUT", "/settings/suggestions")
            .json::<serde_json::Value>(),
        json!(false),
        "Try again sends the choice that failed"
    );
    let texts = test_support::editor::ui::painted_texts(&render_settings(
        &settings,
        &core,
        Rc::new(|_| {}),
    ));
    assert!(!texts.iter().any(|text| text.starts_with("Couldn't save")));
}

#[test]
fn engine_sign_in_can_be_cancelled_while_busy_and_enter_submits_the_code() {
    use aqua_core::{InputEvent, Key, KeyEvent, Modifiers, NamedKey};
    use artisan_api::{EngineId, EngineStatus};
    use editor_aqua::core::{Screen, SettingsPage};
    use editor_aqua::settings::{
        controller::SettingsController,
        view::{ViewAction, dispatch},
    };
    use serde_json::json;
    use std::{cell::RefCell, rc::Rc};
    let dir = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(dir.path());
    let (_silent, id) = silent_host(&mut core, "Desk");
    let engine = EngineId::Codex;
    let status: EngineStatus = serde_json::from_value(json!({
        "engine": engine,
        "install": {"state": "installed", "version": "1.0.0", "update": null},
        "sign_in": {"state": "logging_in", "url": null, "code": null, "accepts_code": true},
        "models": [],
        "default_model": null,
        "usage": null,
    }))
    .unwrap();
    core.host_mut(&id).unwrap().engines.insert(engine, status);
    core.navigate(Screen::Settings(SettingsPage::Engine(engine)));
    let mut settings = SettingsController::default();
    settings.sync(&mut core);
    settings.engines.busy.insert(engine);
    let actions = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&actions);
    let emit: Rc<dyn Fn(ViewAction)> = Rc::new(move |action| sink.borrow_mut().push(action));
    let mut app = render_settings(&settings, &core, Rc::clone(&emit));
    test_support::editor::ui::click(&mut app, &format!("settings-engine-cancel-login-{engine}"));
    for action in actions.borrow_mut().drain(..) {
        dispatch(&mut settings, &mut core, action);
    }
    assert!(settings.engines.cancelling.contains(&engine));
    let texts = test_support::editor::ui::painted_texts(&render_settings(
        &settings,
        &core,
        Rc::clone(&emit),
    ));
    assert!(texts.iter().any(|text| text == "Cancelling…"), "{texts:?}");

    settings.engines.busy.remove(&engine);
    settings
        .engines
        .login_codes
        .get(&engine)
        .unwrap()
        .borrow_mut()
        .set_text("abc-123");
    let mut app = render_settings(&settings, &core, emit);
    assert!(app.focus_key(format!("settings-engine-login-code-{engine}").as_str()));
    let enter = KeyEvent {
        key: Key::Named(NamedKey::Enter),
        modifiers: Modifiers::default(),
        repeat: false,
    };
    app.handle_input(&InputEvent::KeyDown(enter));
    app.handle_input(&InputEvent::KeyUp(enter));
    let submitted: Vec<_> = actions.borrow_mut().drain(..).collect();
    assert!(
        submitted.iter().any(
            |action| matches!(action, ViewAction::EngineCode(submitted) if *submitted == engine)
        ),
        "{submitted:?}"
    );
    for action in submitted {
        dispatch(&mut settings, &mut core, action);
    }
    assert!(settings.engines.busy.contains(&engine));
    assert_eq!(
        settings
            .engines
            .login_codes
            .get(&engine)
            .unwrap()
            .borrow()
            .text(),
        ""
    );
}

#[test]
fn an_mcp_sign_in_that_is_still_opening_can_be_cancelled() {
    use editor_aqua::core::{Screen, SettingsPage};
    use editor_aqua::settings::{controller::SettingsController, view::dispatch};
    use serde_json::json;
    use std::{cell::RefCell, rc::Rc};
    let dir = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(dir.path());
    let (_silent, id) = silent_host(&mut core, "Desk");
    core.host_mut(&id).unwrap().settings.insert(
        "mcp_servers",
        json!([{"name": "linear", "url": "https://mcp.linear.app/mcp"}]),
    );
    core.navigate(Screen::Settings(SettingsPage::Extensions));
    let mut settings = SettingsController::default();
    settings.sync(&mut core);
    let _started = core.start_mcp_sign_in(&id, "linear", false);
    let actions = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&actions);
    let mut app = render_settings(
        &settings,
        &core,
        Rc::new(move |action| sink.borrow_mut().push(action)),
    );
    let texts = test_support::editor::ui::painted_texts(&app);
    assert!(texts.iter().any(|text| text == "Opening…"), "{texts:?}");
    test_support::editor::ui::click(&mut app, "mcp-cancel-sign-in-linear");
    for action in actions.borrow_mut().drain(..) {
        dispatch(&mut settings, &mut core, action);
    }
    assert!(core.mcp_sign_in(&id, "linear").is_none());
}

#[test]
fn signed_in_account_page_opens_the_terms_and_privacy_policy() {
    use editor_aqua::core::{AccountLink, Screen, SettingsPage};
    use editor_aqua::settings::{
        about::BuildInfo,
        controller::SettingsController,
        view::{ViewAction, build},
    };
    use editor_aqua::ui::theme::ArtisanTheme;
    use std::{cell::RefCell, rc::Rc};
    use test_support::editor::ui::{click, mounted};
    let dir = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(dir.path());
    assert!(core.account().profile().is_some());
    core.navigate(Screen::Settings(SettingsPage::Account));
    let controller = SettingsController::default();
    let opened = Rc::new(RefCell::new(Vec::new()));
    for (key, link) in [
        ("account-terms", AccountLink::Terms),
        ("account-privacy", AccountLink::Privacy),
    ] {
        let sink = Rc::clone(&opened);
        let page = build(
            &controller,
            &core,
            &ArtisanTheme::resolved(),
            &BuildInfo::default(),
            aqua_core::Size::new(900.0, 3000.0),
            true,
            Rc::new(move |action| {
                if let ViewAction::OpenUrl(url) = action {
                    sink.borrow_mut().push(url);
                }
            }),
        );
        let mut app = mounted(900.0, 3000.0, page);
        click(&mut app, key);
        assert_eq!(opened.borrow_mut().pop(), Some(link.url()), "{key}");
    }
}

#[test]
fn settings_pages_expose_their_title_and_sections_as_headings() {
    use aqua::UiApp;
    use aqua_core::Role;
    use aqua_text::{SharedFonts, SharedRuns};
    use editor_aqua::core::{Screen, SettingsPage};
    use editor_aqua::settings::{about::BuildInfo, controller::SettingsController, view::build};
    use editor_aqua::ui::theme::ArtisanTheme;
    use std::rc::Rc;

    let dir = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(dir.path());
    let controller = SettingsController::default();
    let mut headings = |page: SettingsPage| {
        core.navigate(Screen::Settings(page));
        let mut element = Some(build(
            &controller,
            &core,
            &ArtisanTheme::resolved(),
            &BuildInfo::default(),
            aqua_core::Size::new(900.0, 2000.0),
            true,
            Rc::new(|_| {}),
        ));
        let mut app = UiApp::new(SharedFonts::new(), SharedRuns::new());
        app.mount(move |_| element.take().expect("the page mounts once"))
            .expect("the settings page mounts");
        app.set_viewport(900.0, 2000.0, 1.0);
        app.build_frame().expect("the settings page lays out");
        app.tree()
            .emit_semantics()
            .into_iter()
            .filter_map(|node| {
                let Role::Heading(level) = node.role else {
                    return None;
                };
                Some((level, node.label))
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        headings(SettingsPage::General),
        [
            (1, "General".to_owned()),
            (2, "Appearance".to_owned()),
            (2, "Text Streaming".to_owned()),
            (2, "Notifications".to_owned()),
            (2, "Performance".to_owned()),
        ]
    );
    for page in [
        SettingsPage::About,
        SettingsPage::Account,
        SettingsPage::Hosts,
    ] {
        let titles: Vec<_> = headings(page)
            .into_iter()
            .filter(|(level, _)| *level == 1)
            .map(|(_, title)| title)
            .collect();
        assert_eq!(titles, [page.label()], "{page:?} has one page title");
    }
}

#[test]
fn general_settings_choose_text_size_and_high_contrast_or_follow_the_system() {
    use editor_aqua::core::{Screen, SettingsPage};
    use editor_aqua::settings::{
        controller::SettingsController,
        view::{ViewAction, dispatch, high_contrast_menu, text_size_menu},
    };
    use editor_aqua::ui::preferences::{Preferences, SystemAccess};
    use editor_aqua::ui::theme::{ArtisanTheme, Contrast};
    use std::{cell::RefCell, rc::Rc};
    use test_support::editor::ui::{click_text, mounted};
    let dir = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(dir.path());
    core.navigate(Screen::Settings(SettingsPage::General));
    let mut settings = SettingsController::default();
    let system = SystemAccess {
        reduced_motion: false,
        high_contrast: true,
        text_scale: 1.25,
    };
    let mut choose = |size: &str, contrast: &str| {
        let actions = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&actions);
        let emit: Rc<dyn Fn(ViewAction)> = Rc::new(move |action| sink.borrow_mut().push(action));
        let theme = ArtisanTheme::resolved();
        let (menu, _) = text_size_menu(&settings, &core, &theme, Rc::clone(&emit)).into_parts();
        click_text(&mut mounted(400.0, 600.0, menu), size);
        let (menu, _) = high_contrast_menu(&settings, &core, &theme, emit).into_parts();
        click_text(&mut mounted(400.0, 600.0, menu), contrast);
        for action in actions.take() {
            assert!(matches!(
                action,
                ViewAction::SetTextScale(_)
                    | ViewAction::SetHighContrast(_)
                    | ViewAction::TextSizeMenu(_)
                    | ViewAction::ContrastMenu(_)
            ));
            dispatch(&mut settings, &mut core, action);
        }
        Preferences::from_core(&core, system).theme()
    };
    let chosen = choose("150%", "Off");
    assert!((chosen.typography.scale - 1.5).abs() < f32::EPSILON);
    assert_eq!(chosen.colors.contrast, Contrast::Standard);
    let followed = choose("Match system", "Match system");
    assert!((followed.typography.scale - 1.25).abs() < f32::EPSILON);
    assert_eq!(followed.colors.contrast, Contrast::High);
}
