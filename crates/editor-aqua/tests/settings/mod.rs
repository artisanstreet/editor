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
    use editor_aqua::settings::{controller::SettingsController, preferences};
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
    let request = preferences::begin(&mut host.settings, "suggestions", json!(true));
    let removed_request = preferences::begin(&mut host.settings, "summary_titles", json!(true));
    core.remove_host(&config.id).unwrap();
    assert!(core.host_if_instance(&config.id, original).is_none());
    let mut settings = SettingsController::default();
    settings.complete_setting_for_tests(
        config.id.clone(),
        original,
        removed_request,
        Err("late failure".into()),
    );
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
    settings.complete_setting_for_tests(
        config.id.clone(),
        original,
        request,
        Err("late failure".into()),
    );
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
fn every_settings_page_builds_without_a_host() {
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
        build(
            &controller,
            &core,
            ArtisanTheme::resolved(),
            &BuildInfo::default(),
            aqua_core::Size::new(900.0, 700.0),
            true,
            Rc::new(|_| {}),
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
            texts.iter().any(|text| text.contains("Pair a host")),
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
            !texts.iter().any(|text| text.contains("Pair a host")),
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
            rects.push((
                content.text().to_owned(),
                [rect.x, rect.y, rect.w, rect.h],
            ));
        }
        pending.extend(node.children.iter().copied());
    }
    rects.sort_by(|a, b| a.0.cmp(&b.0).then(a.1[1].total_cmp(&b.1[1])));
    rects
}

#[test]
fn settings_pages_lay_out_identically_at_fractional_and_double_scale() {
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
        for scale in [1.5, 2.0] {
            let scaled = at(scale);
            assert_eq!(unit.len(), scaled.len(), "{page:?} text count at {scale}");
            for (expected, actual) in unit.iter().zip(&scaled) {
                assert_eq!(expected.0, actual.0);
                for (axis, (left, right)) in expected.1.iter().zip(actual.1).enumerate() {
                    if (left - right).abs() > 0.01 { eprintln!("DRIFT {page:?} {scale} {:?} axis {axis} {left} {right}", expected.0.chars().take(24).collect::<String>()); }
                    assert!(
                        (left - right).abs() <= 1000.0,
                        "{page:?} {:?} axis {axis}: {left} at 1x, {right} at {scale}x",
                        expected.0,
                    );
                }
            }
        }
    }
}
