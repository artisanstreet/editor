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

#[test]
fn mcp_completion_cannot_close_reopened_form() {
    let mut form = ExtensionsForm::default();
    form.sync_host(Some("host".into()));
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
    form.error = Some("host b".into());
    form.complete(&request, Err("old failure".into()), "");
    assert_eq!(form.error.as_deref(), Some("host b"));
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
    use artisan_api::tls::{Identity, fingerprint};
    use editor_aqua::core::{Core, EditorFiles, ForgeClient, HostConfig, HostId, HostKind};
    use editor_aqua::settings::{
        controller::SettingsController,
        preferences::{SettingKey, SettingRequest},
    };
    use serde_json::json;

    let dir = tempfile::tempdir().unwrap();
    let files = EditorFiles {
        dir: dir.path().to_path_buf(),
    };
    let identity = Identity::generate("settings-test").unwrap();
    let fingerprint = fingerprint(identity.certificate_der().unwrap().as_ref());
    let config = HostConfig {
        id: HostId("same-host".into()),
        name: "Test Forge".into(),
        address: "127.0.0.1:1".into(),
        fingerprint: fingerprint.clone(),
        kind: HostKind::Local,
    };
    let client = ForgeClient::new(&config.address, &fingerprint, &identity).unwrap();
    let mut core = Core::for_tests(files).unwrap();
    core.add_test_host(&config, client.clone());
    let original = core.host(&config.id).unwrap().instance();
    let host = core.host_mut(&config.id).unwrap();
    host.settings.insert("suggestions".into(), json!(false));
    host.settings.insert("suggestions".into(), json!(true));
    host.settings.insert("summary_titles".into(), json!(true));
    let request = SettingRequest {
        target: SettingKey {
            host: config.id.clone(),
            instance: original,
            key: "suggestions",
        },
        value: json!(true),
    };
    let removed_request = SettingRequest {
        target: SettingKey {
            host: config.id.clone(),
            instance: original,
            key: "summary_titles",
        },
        value: json!(true),
    };
    core.remove_host(&config.id).unwrap();
    assert!(core.host_if_instance(&config.id, original).is_none());
    let mut settings = SettingsController::default();
    settings.complete_setting_for_tests(removed_request, Err("late failure".into()));
    assert!(settings.drain(&mut core));
    assert!(core.host(&config.id).is_none());
    core.add_test_host(&config, client);
    let host = core.host_mut(&config.id).unwrap();
    assert_ne!(host.instance(), original);
    host.settings
        .insert("suggestions".into(), json!("replacement"));
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
    settings.complete_setting_for_tests(request, Err("late failure".into()));
    assert!(settings.drain(&mut core));
    assert_eq!(
        core.host(&config.id).unwrap().setting("suggestions"),
        Some(&json!("replacement"))
    );
}

#[test]
fn engine_login_code_requires_text_and_clears_only_on_submission() {
    use artisan_api::EngineId;
    use editor_aqua::settings::engines::EnginesForm;
    let mut form = EnginesForm::default();
    form.sync_host(Some("forge".into()));
    form.prepare(EngineId::Claude);
    let editor = form.login_codes.get(&EngineId::Claude).unwrap().clone();
    editor.borrow_mut().set_text("   ");
    assert!(form.login_code(EngineId::Claude).is_none());
    assert_eq!(editor.borrow().text(), "   ");
    assert_eq!(
        form.errors.get(&EngineId::Claude).map(String::as_str),
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
    use editor_aqua::core::{Core, EditorFiles};
    use editor_aqua::settings::{
        controller::SettingsController,
        view::{ViewAction, dispatch},
    };
    use serde_json::json;
    let dir = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: dir.path().to_path_buf(),
    })
    .unwrap();
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
    assert_eq!(core.settings.get("notifications"), Some(&json!(true)));
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
    use editor_aqua::core::{Core, EditorFiles, Screen, SettingsPage, SettingsSection};
    use editor_aqua::settings::{about::BuildInfo, controller::SettingsController, view::build};
    use editor_aqua::ui::theme::ArtisanTheme;
    use std::rc::Rc;
    let dir = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: dir.path().to_path_buf(),
    })
    .unwrap();
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
            ArtisanTheme::resolved(),
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
    app.mount(move |_| element.take().unwrap()).unwrap();
    app.set_viewport(900.0, 700.0, 1.0);
    app.build_frame().unwrap();
    let tree = app.tree();
    let mut pending = vec![tree.root().unwrap()];
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
        Core, EditorFiles, ForgeClient, HostConfig, HostId, HostKind, Screen, SettingsPage,
    };
    use editor_aqua::settings::{about::BuildInfo, controller::SettingsController, view::build};
    use editor_aqua::ui::theme::ArtisanTheme;
    use std::rc::Rc;

    let dir = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: dir.path().to_path_buf(),
    })
    .unwrap();
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
            ArtisanTheme::resolved(),
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
    core.add_test_host(&config, client);
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
    app.mount(move |_| element.take().unwrap()).unwrap();
    app.tree_mut().set_gpui_layout_scale(Some(scale));
    app.set_viewport(900.0, 700.0, scale);
    app.build_frame().unwrap();
    let tree = app.tree();
    let mut pending = vec![tree.root().unwrap()];
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
    use editor_aqua::core::{Core, EditorFiles, Screen, SettingsPage};
    use editor_aqua::settings::{about::BuildInfo, controller::SettingsController, view::build};
    use editor_aqua::ui::theme::ArtisanTheme;
    use std::rc::Rc;

    let dir = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: dir.path().to_path_buf(),
    })
    .unwrap();
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
                    ArtisanTheme::resolved(),
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
        for (scale, position, size) in [(1.5, 4.0, 2.0), (2.0, 0.5, 0.5)] {
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
    let identity = Identity::generate("settings-host").unwrap();
    let fingerprint = fingerprint(identity.certificate_der().unwrap().as_ref());
    let config = HostConfig {
        id: HostId(id.into()),
        name: id.into(),
        address: "127.0.0.1:1".into(),
        fingerprint: fingerprint.clone(),
        kind: HostKind::Local,
    };
    let client = ForgeClient::new(&config.address, &fingerprint, &identity).unwrap();
    core.add_test_host(&config, client);
    config.id
}

fn retention_target(
    core: &mut editor_aqua::core::Core,
) -> editor_aqua::settings::preferences::SettingKey {
    let id = add_host(core, "writes-host");
    editor_aqua::settings::preferences::SettingKey {
        instance: core.host(&id).unwrap().instance(),
        host: id,
        key: artisan_api::THREAD_RETENTION_DAYS_KEY,
    }
}

#[test]
fn an_older_failed_setting_never_rolls_back_a_newer_choice() {
    use editor_aqua::core::{Core, EditorFiles};
    use editor_aqua::settings::preferences::SettingWrites;
    use serde_json::json;
    let dir = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: dir.path().to_path_buf(),
    })
    .unwrap();
    let target = retention_target(&mut core);
    let key = target.key;
    let mut settings = editor_aqua::core::HostSettings::new(target.host.clone());
    settings.insert(key.into(), json!(90));
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
        .complete(Some(&mut settings), &seven, &Err("offline".into()))
        .expect("the newer choice is sent after the older one finishes");
    assert_eq!(settings.get(key), Some(&json!(14)));
    assert!(
        writes
            .complete(Some(&mut settings), &fourteen, &Ok(()))
            .is_none()
    );
    assert_eq!(settings.get(key), Some(&json!(14)));

    let thirty = writes
        .begin(&mut settings, target.clone(), json!(30))
        .expect("sent");
    assert!(
        writes
            .complete(Some(&mut settings), &thirty, &Err("offline".into()))
            .is_none()
    );
    assert_eq!(
        settings.get(key),
        Some(&json!(14)),
        "a failure restores the last value Forge accepted"
    );

    let sixty = writes
        .begin(&mut settings, target, json!(60))
        .expect("sent");
    settings.insert(key.into(), json!(45));
    assert!(
        writes
            .complete(Some(&mut settings), &sixty, &Err("offline".into()))
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
    use editor_aqua::core::{Core, EditorFiles, Screen, SettingsPage};
    use editor_aqua::settings::controller::SettingsController;
    use std::sync::Arc;
    let dir = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: dir.path().to_path_buf(),
    })
    .unwrap();
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
    use editor_aqua::core::{Core, EditorFiles, Screen, SettingsPage};
    use editor_aqua::settings::controller::SettingsController;
    let dir = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: dir.path().to_path_buf(),
    })
    .unwrap();
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
    use editor_aqua::core::{Core, EditorFiles, Screen, SettingsPage};
    use editor_aqua::settings::{
        about::BuildInfo,
        controller::SettingsController,
        view::{ViewAction, build},
    };
    use editor_aqua::ui::theme::ArtisanTheme;
    use std::{cell::RefCell, rc::Rc};
    let dir = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: dir.path().to_path_buf(),
    })
    .unwrap();
    core.navigate(Screen::Settings(SettingsPage::About));
    let controller = SettingsController::default();
    let emitted = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&emitted);
    let page = build(
        &controller,
        &core,
        ArtisanTheme::resolved(),
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
    let silent = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let identity = Identity::generate("settings-silent-host").unwrap();
    let fingerprint = fingerprint(identity.certificate_der().unwrap().as_ref());
    let config = HostConfig {
        id: HostId(id.into()),
        name: id.into(),
        address: silent.local_addr().unwrap().to_string(),
        fingerprint: fingerprint.clone(),
        kind: HostKind::Local,
    };
    let client = ForgeClient::new(&config.address, &fingerprint, &identity).unwrap();
    core.add_test_host(&config, client);
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
        ArtisanTheme::resolved(),
        &BuildInfo::default(),
        aqua_core::Size::new(900.0, 2400.0),
        true,
        emit,
    );
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| view).unwrap();
    app.set_viewport(900.0, 2400.0, 1.0);
    app.build_frame().unwrap();
    app
}

#[test]
fn a_failed_forge_setting_names_the_setting_and_its_cause_and_tries_again() {
    use editor_aqua::core::{Core, EditorFiles, Screen, SettingsPage};
    use editor_aqua::settings::{
        controller::SettingsController,
        preferences::{SettingKey, SettingRequest},
        view::{ViewAction, dispatch},
    };
    use serde_json::json;
    use std::{cell::RefCell, rc::Rc};
    let dir = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: dir.path().to_path_buf(),
    })
    .unwrap();
    let (_silent, id) = silent_host(&mut core, "Desk");
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
    settings.complete_setting_for_tests(
        SettingRequest {
            target: target.clone(),
            value: json!(false),
        },
        Err("the host did not answer".into()),
    );
    assert!(settings.drain(&mut core));
    assert_ne!(
        core.host(&id).unwrap().setting("suggestions"),
        Some(&json!(false))
    );
    let actions = Rc::new(RefCell::new(Vec::new()));
    let sink = actions.clone();
    let mut app = render_settings(
        &settings,
        &core,
        Rc::new(move |action| sink.borrow_mut().push(action)),
    );
    let texts = test_support::editor::ui::painted_texts(&app);
    assert!(
        texts.iter().any(|text| text
            == "Couldn't save “Suggest the next message” on Desk: the host did not answer"),
        "{texts:?}"
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
    use editor_aqua::core::{Core, EditorFiles, Screen, SettingsPage};
    use editor_aqua::settings::{
        controller::SettingsController,
        view::{ViewAction, dispatch},
    };
    use serde_json::json;
    use std::{cell::RefCell, rc::Rc};
    let dir = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: dir.path().to_path_buf(),
    })
    .unwrap();
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
    let sink = actions.clone();
    let emit: Rc<dyn Fn(ViewAction)> = Rc::new(move |action| sink.borrow_mut().push(action));
    let mut app = render_settings(&settings, &core, emit.clone());
    test_support::editor::ui::click(&mut app, &format!("settings-engine-cancel-login-{engine}"));
    for action in actions.borrow_mut().drain(..) {
        dispatch(&mut settings, &mut core, action);
    }
    assert!(settings.engines.cancelling.contains(&engine));
    let texts =
        test_support::editor::ui::painted_texts(&render_settings(&settings, &core, emit.clone()));
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
    use editor_aqua::core::{Core, EditorFiles, Screen, SettingsPage};
    use editor_aqua::settings::{controller::SettingsController, view::dispatch};
    use serde_json::json;
    use std::{cell::RefCell, rc::Rc};
    let dir = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: dir.path().to_path_buf(),
    })
    .unwrap();
    let (_silent, id) = silent_host(&mut core, "Desk");
    core.host_mut(&id).unwrap().settings.insert(
        "mcp_servers".into(),
        json!([{"name": "linear", "url": "https://mcp.linear.app/mcp"}]),
    );
    core.navigate(Screen::Settings(SettingsPage::Extensions));
    let mut settings = SettingsController::default();
    settings.sync(&mut core);
    let _started = core.start_mcp_sign_in(&id, "linear", false);
    let actions = Rc::new(RefCell::new(Vec::new()));
    let sink = actions.clone();
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
    use editor_aqua::core::{AccountLink, Core, EditorFiles, Screen, SettingsPage};
    use editor_aqua::settings::{
        about::BuildInfo,
        controller::SettingsController,
        view::{ViewAction, build},
    };
    use editor_aqua::ui::theme::ArtisanTheme;
    use std::{cell::RefCell, rc::Rc};
    use test_support::editor::ui::{click, mounted};
    let dir = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: dir.path().to_path_buf(),
    })
    .unwrap();
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
            ArtisanTheme::resolved(),
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
