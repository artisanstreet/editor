use std::time::{Duration, Instant};

use artisan_api::Listing;
use editor_aqua::core::HostId;
use editor_aqua::marketplace::presentation::{self, Item, ServerRow};
use editor_aqua::marketplace::{Marketplace, Remote};

fn listing(id: &str, name: &str, installs: u64) -> Listing {
    Listing {
        id: id.into(),
        name: name.into(),
        publisher: id.into(),
        source: "skills.sh".into(),
        installs,
    }
}

#[test]
fn rank_and_deduplicate_listings() {
    let listings = vec![
        listing("first", "pdf", 1),
        listing("other", "csv", 3),
        listing("best", "pdf", 10),
    ];
    let named = presentation::by_name(&listings);
    assert_eq!(
        named
            .iter()
            .map(|(listing, count)| (listing.id.as_str(), *count))
            .collect::<Vec<_>>(),
        vec![("best", 1), ("other", 0)]
    );
    let items = vec![
        Item::Server(ServerRow {
            name: "zpdf".into(),
            target: "target".into(),
        }),
        Item::Server(ServerRow {
            name: "pdf-first".into(),
            target: "target".into(),
        }),
        Item::Server(ServerRow {
            name: "other".into(),
            target: "pdf target".into(),
        }),
    ];
    assert_eq!(
        presentation::search(&items, " PDF ")
            .iter()
            .map(Item::name)
            .collect::<Vec<_>>(),
        vec!["pdf-first", "zpdf", "other"]
    );
}

#[test]
fn debounce_and_reject_stale_searches() {
    let mut page = Marketplace::default();
    page.set_shown(true);
    page.sync_host(Some(HostId("host".into())), &[], &[]);
    let now = Instant::now();
    page.set_query("pdf".into(), now);
    assert!(page.take_search(now + Duration::from_millis(249)).is_none());
    let old = page.take_search(now + Duration::from_millis(250)).unwrap();
    page.set_query("pdfs".into(), now + Duration::from_millis(251));
    page.finish_search(&old, Ok(vec![listing("old", "pdf", 1)]));
    assert!(matches!(page.listings, Remote::Searching));
    let current = page.take_search(now + Duration::from_millis(501)).unwrap();
    page.finish_search(&current, Ok(vec![listing("new", "pdfs", 1)]));
    assert!(matches!(page.listings, Remote::Found(ref listings) if listings[0].id == "new"));
}

#[test]
fn changing_host_discards_old_operation_completion() {
    use artisan_api::SkillAction;
    use editor_aqua::marketplace::Operation;

    let mut page = Marketplace::default();
    page.set_shown(true);
    page.sync_host(Some(HostId("first".into())), &[], &[]);
    let request = page
        .begin_operation(Operation::Skill {
            id: "skill".into(),
            action: SkillAction::Disable,
        })
        .unwrap();
    assert!(
        page.begin_operation(Operation::Listing("another".into()))
            .is_none()
    );
    page.sync_host(Some(HostId("second".into())), &[], &[]);
    assert!(!page.finish_operation(&request, Err(anyhow::anyhow!("failed"))));
    assert!(page.pending.is_none());
    assert!(page.notice.is_none());
}

#[test]
fn removed_and_readded_host_rejects_old_requests() {
    use artisan_api::SkillAction;
    use editor_aqua::marketplace::Operation;

    let host = HostId("same-host".into());
    let mut page = Marketplace::default();
    page.set_shown(true);
    page.sync_host(Some(host.clone()), &[], &[]);
    let now = Instant::now();
    page.set_query("pdf".into(), now);
    let search = page.take_search(now + Duration::from_millis(250)).unwrap();
    let operation = page
        .begin_operation(Operation::Skill {
            id: "skill".into(),
            action: SkillAction::Adopt,
        })
        .unwrap();
    page.sync_host(None, &[], &[]);
    page.cancel_search();
    page.sync_host(Some(host), &[], &[]);
    page.finish_search(&search, Ok(vec![listing("old", "pdf", 10)]));
    assert_eq!(page.listings, Remote::Idle);
    assert!(!page.finish_operation(&operation, Err(anyhow::anyhow!("old error"))));
    assert!(page.pending.is_none());
    assert!(page.notice.is_none());
    assert!(
        page.begin_operation(Operation::Listing("new".into()))
            .is_some()
    );
}

#[test]
fn same_id_host_replacement_resets_marketplace_requests() {
    use artisan_api::SkillAction;
    use artisan_api::tls::{Identity, fingerprint};
    use editor_aqua::core::{Core, EditorFiles, ForgeClient, HostConfig, HostKind, Screen};
    use editor_aqua::marketplace::Operation;
    use editor_aqua::marketplace::controller::MarketplaceController;

    let directory = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: directory.path().join("editor"),
    })
    .unwrap();
    let identity = Identity::generate("marketplace-test").unwrap();
    let fingerprint = fingerprint(identity.certificate_der().unwrap().as_ref());
    let client = ForgeClient::new("127.0.0.1:1", &fingerprint, &identity).unwrap();
    let id = HostId("same-host".into());
    let config = HostConfig {
        id: id.clone(),
        name: "Forge".into(),
        address: "127.0.0.1:1".into(),
        fingerprint,
        kind: HostKind::Local,
    };
    core.add_test_host(&config, client.clone());
    core.navigate(Screen::Marketplace);
    let mut controller = MarketplaceController::default();
    controller.sync(&mut core);
    let first = core.host(&id).unwrap().instance();
    let now = Instant::now();
    controller.page.set_query("pdf".into(), now);
    let search = controller
        .page
        .take_search(now + Duration::from_millis(250))
        .unwrap();
    let operation = controller
        .page
        .begin_operation(Operation::Skill {
            id: "skill".into(),
            action: SkillAction::Adopt,
        })
        .unwrap();
    core.remove_host(&id).unwrap();
    assert!(core.host_if_instance(&id, first).is_none());
    core.add_test_host(&config, client);
    assert_ne!(core.host(&id).unwrap().instance(), first);
    assert!(core.host_if_instance(&id, first).is_none());
    controller.sync(&mut core);
    controller
        .page
        .finish_search(&search, Ok(vec![listing("old", "pdf", 10)]));
    assert_eq!(controller.page.listings, Remote::Idle);
    assert!(
        !controller
            .page
            .finish_operation(&operation, Err(anyhow::anyhow!("old error")))
    );
    assert!(controller.page.pending.is_none());
    assert!(controller.page.notice.is_none());
    assert!(
        controller
            .page
            .begin_operation(Operation::Listing("new".into()))
            .is_some()
    );
}

#[test]
fn mounted_search_routes_keyboard_selection_and_escape() {
    use aqua_core::{InputEvent, Key as InputKey, KeyEvent, Modifiers, NamedKey};
    use aqua_text::{SharedFonts, SharedRuns};
    use aqua_ui::{element::Key, tree::hash_key};
    use artisan_api::tls::{Identity, fingerprint};
    use editor_aqua::core::{
        Core, EditorFiles, ForgeClient, HostConfig, HostKind, Screen, SettingsPage,
    };
    use editor_aqua::marketplace::{controller::MarketplaceController, view};
    use editor_aqua::ui::theme::ArtisanTheme;
    use std::cell::RefCell;
    use std::rc::Rc;

    let directory = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: directory.path().join("editor"),
    })
    .unwrap();
    let identity = Identity::generate("marketplace-view-test").unwrap();
    let fingerprint = fingerprint(identity.certificate_der().unwrap().as_ref());
    let client = ForgeClient::new("127.0.0.1:1", &fingerprint, &identity).unwrap();
    let id = HostId("view-host".into());
    core.add_test_host(
        &HostConfig {
            id,
            name: "Forge".into(),
            address: "127.0.0.1:1".into(),
            fingerprint,
            kind: HostKind::Local,
        },
        client,
    );
    core.navigate(Screen::Marketplace);
    let events = Rc::new(RefCell::new(Vec::new()));
    let recorded = events.clone();
    let emit = Rc::new(move |action| recorded.borrow_mut().push(action));
    let mut controller = MarketplaceController::new(emit.clone());
    controller.sync(&mut core);
    controller.page.items = vec![
        Item::Server(ServerRow {
            name: "pdf".into(),
            target: "first".into(),
        }),
        Item::Server(ServerRow {
            name: "print".into(),
            target: "second".into(),
        }),
    ];
    assert!(controller.take_focus_request());
    assert!(!view::dispatch(
        &mut controller,
        &mut core,
        view::ViewAction::SearchChanged
    ));
    assert!(!view::dispatch(
        &mut controller,
        &mut core,
        view::ViewAction::SetKind(presentation::Kind::All)
    ));
    assert!(!view::dispatch(
        &mut controller,
        &mut core,
        view::ViewAction::SearchKey(editor_aqua::ui::text::ListKey::Up)
    ));
    assert!(!controller.drain(&mut core));
    assert!(!view::dispatch(
        &mut controller,
        &mut core,
        view::ViewAction::FocusSearch
    ));
    assert!(controller.take_focus_request());
    assert!(view::dispatch(
        &mut controller,
        &mut core,
        view::ViewAction::PromptHover(true)
    ));
    assert!(!view::dispatch(
        &mut controller,
        &mut core,
        view::ViewAction::PromptHover(true)
    ));
    assert!(view::dispatch(
        &mut controller,
        &mut core,
        view::ViewAction::PromptHover(false)
    ));
    assert!(view::dispatch(
        &mut controller,
        &mut core,
        view::ViewAction::ToggleFound
    ));
    assert!(view::dispatch(
        &mut controller,
        &mut core,
        view::ViewAction::ToggleFound
    ));
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| view::build(&controller, &core, ArtisanTheme::resolved(), emit))
        .unwrap();
    app.set_viewport(1000.0, 900.0, 1.0);
    app.build_frame().unwrap();
    for selector in ["marketplace-kind-Skills", "marketplace-global-prompt-open"] {
        let control = app.find_debug_selector(selector).unwrap();
        let text = app.tree().get(control).unwrap().children[0];
        let text_color = |app: &aqua::UiApp| match &app.tree().get(text).unwrap().kind {
            aqua_ui::tree::NodeKind::Text(text) => text.text_style.gpui_source_color,
            _ => panic!("control label is not text"),
        };
        let muted = ArtisanTheme::resolved()
            .colors
            .text_muted
            .to_reference_text_color();
        assert_eq!(
            text_color(&app),
            Some([
                muted.hue_degrees,
                muted.saturation,
                muted.lightness,
                muted.alpha
            ])
        );
        let bounds = app.tree().rect(control).unwrap();
        app.handle_input(&InputEvent::PointerMove(aqua_core::PointerEvent {
            contact: 0,
            kind: aqua_core::PointerKind::Mouse,
            position: aqua_core::Point::new(bounds.x + bounds.w / 2.0, bounds.y + bounds.h / 2.0),
            buttons: 0,
        }));
        app.build_frame().unwrap();
        let foreground = ArtisanTheme::resolved()
            .colors
            .text
            .to_reference_text_color();
        assert_eq!(
            text_color(&app),
            Some([
                foreground.hue_degrees,
                foreground.saturation,
                foreground.lightness,
                foreground.alpha
            ])
        );
        app.handle_input(&InputEvent::PointerMove(aqua_core::PointerEvent {
            contact: 0,
            kind: aqua_core::PointerKind::Mouse,
            position: aqua_core::Point::new(0.0, 0.0),
            buttons: 0,
        }));
        app.build_frame().unwrap();
        assert_eq!(
            text_color(&app),
            Some([
                muted.hue_degrees,
                muted.saturation,
                muted.lightness,
                muted.alpha
            ])
        );
    }
    let key = hash_key(Key::from("marketplace-search"));
    let search = app
        .tree()
        .traverse()
        .into_iter()
        .find(|id| {
            app.tree()
                .get(*id)
                .is_some_and(|node| node.key == Some(key))
        })
        .unwrap();
    assert!(app.focus_key("marketplace-search"));
    assert_eq!(app.tree().focused(), Some(search));
    let bounds = app.tree().rect(search).unwrap();
    app.tree_mut().set_focused(None);
    app.handle_input(&InputEvent::PointerDown(
        aqua_core::PointerEvent {
            contact: 0,
            kind: aqua_core::PointerKind::Mouse,
            position: aqua_core::Point::new(bounds.x - 8.0, bounds.y + bounds.h / 2.0),
            buttons: 1,
        },
        aqua_core::PointerButton::Primary,
    ));
    for action in events.borrow_mut().drain(..) {
        view::dispatch(&mut controller, &mut core, action);
    }
    assert!(controller.take_focus_request());
    assert!(app.focus_key("marketplace-search"));
    assert_eq!(app.tree().focused(), Some(search));
    app.handle_input(&InputEvent::TextInput { text: "p".into() });
    for action in events.borrow_mut().drain(..) {
        view::dispatch(&mut controller, &mut core, action);
    }
    assert_eq!(controller.page.query, "p");
    assert!(controller.page.search_deadline().is_none());
    assert_eq!(controller.page.selected, 0);
    let key_event = |key| {
        InputEvent::KeyDown(KeyEvent {
            key: InputKey::Named(key),
            modifiers: Modifiers::NONE,
            repeat: false,
        })
    };
    app.handle_input(&key_event(NamedKey::Down));
    for action in events.borrow_mut().drain(..) {
        view::dispatch(&mut controller, &mut core, action);
    }
    assert_eq!(controller.page.selected, 1);
    assert!(controller.selected_entry_key().is_some());
    app.handle_input(&key_event(NamedKey::Escape));
    for action in events.borrow_mut().drain(..) {
        view::dispatch(&mut controller, &mut core, action);
    }
    assert!(controller.page.query.is_empty());
    assert_eq!(controller.page.selected, 0);
    app.handle_input(&InputEvent::TextInput { text: "p".into() });
    for action in events.borrow_mut().drain(..) {
        view::dispatch(&mut controller, &mut core, action);
    }
    app.handle_input(&key_event(NamedKey::Enter));
    for action in events.borrow_mut().drain(..) {
        view::dispatch(&mut controller, &mut core, action);
    }
    assert_eq!(core.screen(), &Screen::Settings(SettingsPage::Extensions));
}
