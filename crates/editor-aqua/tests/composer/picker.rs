use artisan_api::{EngineConfig, EngineId, Model};
use editor_aqua::composer::picker::{Axis, AxisKey, AxisKeyResult, PickerState};

#[test]
fn keyboard_selection_uses_filtered_options_and_keeps_existing_hidden_choices() {
    let mut picker = PickerState::default();
    picker.hide_bad_model_options = true;
    let model = Model {
        id: "claude-opus-5-5".into(),
        efforts: ["low", "medium", "high", "xhigh", "max"]
            .map(str::to_owned)
            .into(),
        ..Model::default()
    };
    let config = EngineConfig::new(EngineId::Claude);
    let options = picker.axis_options(Axis::Thinking, &config, Some(&model));
    assert_eq!(options.len(), 2);
    picker.axis_key(AxisKey::End, 0, options.len());
    let AxisKeyResult::Choose(index) = picker.axis_key(AxisKey::Accept, 0, options.len()) else {
        panic!("expected a choice")
    };
    assert_eq!(options[index].apply.effort.as_deref(), Some("high"));
    let config = EngineConfig {
        effort: Some("max".into()),
        ..config
    };
    let options = picker.axis_options(Axis::Thinking, &config, Some(&model));
    let selected = options.iter().find(|option| option.selected).unwrap();
    assert_eq!(selected.apply.effort.as_deref(), Some("max"));
    assert!(
        selected
            .description
            .as_deref()
            .unwrap()
            .contains("hidden by preference")
    );
}

#[test]
fn axes_offer_only_concrete_choices_and_resolve_inherited_selections() {
    use artisan_api::{ContextWindow, Speed};
    let picker = PickerState::default();
    let model = Model {
        efforts: vec!["low".into(), "medium".into(), "high".into()],
        default_effort: Some("low".into()),
        speeds: vec![
            Speed {
                id: "standard".into(),
                label: "Normal".into(),
                description: None,
                default: true,
            },
            Speed {
                id: "fast".into(),
                label: "Fast".into(),
                description: None,
                default: false,
            },
        ],
        context_windows: vec![ContextWindow {
            tokens: 500_000,
            override_tokens: None,
        }],
        ..Model::default()
    };
    let config = EngineConfig::new(EngineId::Codex);
    let coverage = Model {
        efforts: vec!["medium".into(), "ultra".into()],
        variants: ["none", "thinking", "creative"]
            .map(|id| artisan_api::Variant {
                id: id.into(),
                label: id.into(),
            })
            .into(),
        context_window: Some(1_000_000),
        ..model.clone()
    };
    let claude = EngineConfig::new(EngineId::Claude);
    for axis in Axis::PICKER {
        let options = picker.axis_options(axis, &claude, Some(&coverage));
        assert!(!options.is_empty(), "missing coverage for {axis:?}");
        assert!(
            options
                .iter()
                .all(|option| !option.label.starts_with("Default"))
        );
        assert!(options.iter().all(|option| option.description.as_deref()
            != Some("Use the engine default without pinning a value.")));
    }
    for (axis, expected, count) in [
        (Axis::Thinking, "Light", 3),
        (Axis::Speed, "Normal", 2),
        (Axis::Context, "500K", 1),
    ] {
        let options = picker.axis_options(axis, &config, Some(&model));
        assert_eq!(options.len(), count);
        let selected = options.iter().find(|option| option.selected).unwrap();
        assert_eq!(selected.label, expected);
        assert_eq!(selected.apply.variant, config.variant);
        assert_eq!(selected.apply.permissions, config.permissions);
    }
    let selected = picker
        .axis_options(Axis::Thinking, &config, Some(&model))
        .remove(0)
        .apply;
    assert_eq!(selected.effort.as_deref(), Some("low"));
    assert_eq!(selected.speed, None);
    assert_eq!(selected.options["thinking_level"], "light");
    let explicit = EngineConfig {
        effort: Some("high".into()),
        ..config
    };
    let options = picker.axis_options(Axis::Thinking, &explicit, Some(&model));
    assert_eq!(
        options.iter().find(|option| option.selected).unwrap().label,
        "High"
    );
}

fn picker_host(
    name: &str,
) -> (
    tempfile::TempDir,
    editor_aqua::core::Core,
    editor_aqua::core::HostId,
) {
    use artisan_api::tls::{Identity, fingerprint};
    use editor_aqua::core::{ForgeClient, HostConfig, HostId, HostKind};

    let directory = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(&directory.path().join("editor"));
    let identity = Identity::generate(name).unwrap();
    let print = fingerprint(identity.certificate_der().unwrap().as_ref());
    let client = ForgeClient::new("127.0.0.1:1", &print, &identity).unwrap();
    let host = HostId(name.into());
    test_support::editor::core::connect_client(
        &mut core,
        &HostConfig {
            id: host.clone(),
            name: "Forge".into(),
            address: "127.0.0.1:1".into(),
            fingerprint: print,
            kind: HostKind::Local,
        },
        client,
    );
    (directory, core, host)
}

#[test]
fn the_model_picker_offers_install_and_sign_in_for_engines_that_are_not_ready() {
    use artisan_api::{DiscoveryState, EngineStatus, InstallState, SignInState};
    use editor_aqua::composer::controller::ComposerTarget;
    use editor_aqua::composer::picker::Menu;
    use editor_aqua::composer::view::ComposerOwner;
    use editor_aqua::ui::theme::ArtisanTheme;

    let (_directory, mut core, host) = picker_host("picker-gates");
    let status = |engine, install, sign_in| EngineStatus {
        discovery: DiscoveryState::Unknown,
        engine,
        install,
        sign_in,
        models: Vec::new().into(),
        default_model: None,
        commands: Vec::new().into(),
        usage: None,
        integrity: None,
        versions: Vec::new().into(),
        claude_ai: None,
    };
    let installed = || InstallState::Installed {
        version: "1.0.0".into(),
        update: None,
    };
    let engines = &mut core.host_mut(&host).unwrap().engines;
    for engine in [
        status(
            EngineId::Claude,
            InstallState::Absent,
            SignInState::SignedOut,
        ),
        status(EngineId::Codex, installed(), SignInState::SignedOut),
        status(
            EngineId::OpenCode,
            InstallState::Verifying {
                version: "2.0.0".into(),
            },
            SignInState::Unknown,
        ),
    ] {
        engines.insert(engine.engine, engine);
    }
    let (_spool, mut sessions) = crate::draft_sessions();
    let owner = ComposerOwner::new(
        &core,
        &mut sessions,
        ComposerTarget::NewThread {
            host: host.clone(),
            project: None,
        },
        std::rc::Rc::new(|_| {}),
    );
    crate::recovered(owner.controller.borrow().session.as_ref().unwrap());
    owner
        .controller
        .borrow_mut()
        .open_picker(Menu::Model, core.host(&host).unwrap());
    let menu = owner
        .model_menu(
            &ArtisanTheme::resolved(),
            core.host(&host).unwrap(),
            aqua_core::Size::new(900.0, 700.0),
            0.0,
            &aqua::images::ImageAssets::default(),
            std::time::Instant::now(),
        )
        .unwrap();
    let app = test_support::editor::ui::mounted(900.0, 700.0, menu);
    let texts = test_support::editor::ui::painted_texts(&app);
    for wanted in [
        "Install Claude Code",
        "Sign in to Codex",
        "OpenCode is not ready: installing.",
    ] {
        assert!(
            texts.iter().any(|text| text == wanted),
            "the picker shows {wanted:?} among {texts:?}"
        );
    }
}

fn largest_text() -> editor_aqua::ui::theme::ArtisanTheme {
    use editor_aqua::ui::preferences::{Preferences, StreamingMode, SystemAccess};
    use editor_aqua::ui::theme::{FontSet, ThemeMode};
    Preferences {
        theme: ThemeMode::Dark,
        fonts: FontSet::Spline,
        outline_text: true,
        reduced_motion: None,
        high_contrast: None,
        text_scale: Some(2.25),
        streaming: StreamingMode::Smooth,
        lower_frame_rate: false,
        vsync: true,
        system: SystemAccess::default(),
    }
    .theme()
}

fn every_setting_engine() -> artisan_api::EngineStatus {
    use artisan_api::{DiscoveryState, EngineStatus, InstallState, SignInState};
    let model = Model {
        id: "every-setting".into(),
        name: "Every setting".into(),
        efforts: vec!["medium".into(), "ultra".into()],
        variants: ["none", "thinking"]
            .map(|id| artisan_api::Variant {
                id: id.into(),
                label: id.into(),
            })
            .into(),
        speeds: vec![
            artisan_api::Speed {
                id: "standard".into(),
                label: "Normal".into(),
                description: None,
                default: true,
            },
            artisan_api::Speed {
                id: "fast".into(),
                label: "Fast".into(),
                description: None,
                default: false,
            },
        ],
        context_window: Some(1_000_000),
        ..Model::default()
    };
    EngineStatus {
        discovery: DiscoveryState::Unknown,
        engine: EngineId::Claude,
        install: InstallState::Installed {
            version: "1.0.0".into(),
            update: None,
        },
        sign_in: SignInState::SignedIn { account: None },
        models: vec![model].into(),
        default_model: None,
        commands: Vec::new().into(),
        usage: None,
        integrity: None,
        versions: Vec::new().into(),
        claude_ai: None,
    }
}

#[test]
fn the_model_menus_setting_rows_keep_their_full_height_at_the_largest_text_size() {
    use editor_aqua::composer::controller::ComposerTarget;
    use editor_aqua::composer::picker::Menu;
    use editor_aqua::composer::view::ComposerOwner;
    use editor_aqua::ui::kit::{Density, Row};

    let theme = largest_text();
    let (_directory, mut core, host) = picker_host("picker-largest-text");
    core.host_mut(&host)
        .unwrap()
        .engines
        .insert(EngineId::Claude, every_setting_engine());
    let (_spool, mut sessions) = crate::draft_sessions();
    let owner = ComposerOwner::new(
        &core,
        &mut sessions,
        ComposerTarget::NewThread {
            host: host.clone(),
            project: None,
        },
        std::rc::Rc::new(|_| {}),
    );
    {
        let mut composer = owner.controller.borrow_mut();
        composer.config = Some(EngineConfig {
            model: Some("every-setting".into()),
            ..EngineConfig::new(EngineId::Claude)
        });
        composer.open_picker(Menu::Model, core.host(&host).unwrap());
    }
    let menu = owner
        .model_menu(
            &theme,
            core.host(&host).unwrap(),
            aqua_core::Size::new(1400.0, 1400.0),
            0.0,
            &aqua::images::ImageAssets::default(),
            std::time::Instant::now(),
        )
        .unwrap();
    let app = test_support::editor::ui::mounted(1400.0, 1400.0, menu);
    let tree = app.tree();
    let rows: Vec<_> = (0..)
        .map(|position| format!("composer-pane-{position}"))
        .map_while(|key| {
            let hash = aqua_ui::tree::hash_key(aqua_ui::element::Key::from(key.as_str()));
            tree.traverse()
                .into_iter()
                .find(|node| {
                    tree.get(*node)
                        .is_some_and(|record| record.key == Some(hash))
                })
                .and_then(|node| tree.rect(node))
        })
        .collect();
    assert!(
        rows.len() >= 5,
        "the model offers several settings: {rows:?}"
    );
    let tall = Row::ListItem(Density::Tall).height(&theme);
    for row in &rows {
        assert!(
            row.h + 0.5 >= tall,
            "a setting row is as tall as its two lines at 225%: {row:?} against {tall}"
        );
    }
    for pair in rows.windows(2) {
        assert!(
            pair[0].y + pair[0].h <= pair[1].y + 0.5,
            "setting rows don't overlap: {pair:?}"
        );
    }
}
