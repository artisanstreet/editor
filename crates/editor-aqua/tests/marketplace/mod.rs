use std::time::{Duration, Instant};

use artisan_api::Listing;
use editor_aqua::core::HostId;
use editor_aqua::core::client::{CallError, CallFailure};
use editor_aqua::marketplace::presentation::{self, Item, ServerRow};
use editor_aqua::marketplace::{Marketplace, OperationError, Remote};

fn refused() -> OperationError {
    OperationError::Install {
        listing: "listing".into(),
        source: CallError {
            method: "POST".parse().expect("POST is an HTTP method"),
            path: "/marketplace/install".into(),
            failure: Box::new(CallFailure::Inconsistent { check: "test" }),
        },
    }
}

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
    assert!(!page.finish_operation(&request, Err(refused())));
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
    assert!(!page.finish_operation(&operation, Err(refused())));
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
    use editor_aqua::core::{ForgeClient, HostConfig, HostKind, Screen};
    use editor_aqua::marketplace::Operation;
    use editor_aqua::marketplace::controller::MarketplaceController;

    let directory = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(&directory.path().join("editor"));
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
    test_support::editor::core::connect_client(&mut core, &config, client.clone());
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
    test_support::editor::core::connect_client(&mut core, &config, client);
    assert_ne!(core.host(&id).unwrap().instance(), first);
    assert!(core.host_if_instance(&id, first).is_none());
    controller.sync(&mut core);
    controller
        .page
        .finish_search(&search, Ok(vec![listing("old", "pdf", 10)]));
    assert_eq!(controller.page.listings, Remote::Searching);
    assert!(!controller.page.finish_operation(&operation, Err(refused())));
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
#[expect(
    clippy::too_many_lines,
    reason = "one keyboard search scenario walked step by step"
)]
fn mounted_search_routes_keyboard_selection_and_escape() {
    use aqua_core::{InputEvent, Key as InputKey, KeyEvent, Modifiers, NamedKey};
    use aqua_text::{SharedFonts, SharedRuns};
    use aqua_ui::{element::Key, tree::hash_key};
    use artisan_api::tls::{Identity, fingerprint};
    use editor_aqua::core::{ForgeClient, HostConfig, HostKind, Screen, SettingsPage};
    use editor_aqua::marketplace::{controller::MarketplaceController, view};
    use editor_aqua::ui::theme::ArtisanTheme;
    use std::cell::RefCell;
    use std::rc::Rc;

    let directory = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(&directory.path().join("editor"));
    let identity = Identity::generate("marketplace-view-test").unwrap();
    let fingerprint = fingerprint(identity.certificate_der().unwrap().as_ref());
    let client = ForgeClient::new("127.0.0.1:1", &fingerprint, &identity).unwrap();
    let id = HostId("view-host".into());
    test_support::editor::core::connect_client(
        &mut core,
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
    let recorded = Rc::clone(&events);
    let emit: Rc<dyn Fn(view::ViewAction)> =
        Rc::new(move |action| recorded.borrow_mut().push(action));
    let mut controller = MarketplaceController::new(Rc::clone(&emit));
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
    let fonts = SharedFonts::new();
    editor_aqua::ui::fonts::register_bundled_fonts(&fonts);
    let mut app = aqua::UiApp::new(fonts, SharedRuns::new());
    app.mount(|_| view::build(&controller, &core, &ArtisanTheme::resolved(), emit))
        .unwrap();
    app.set_viewport(1000.0, 900.0, 1.0);
    app.build_frame().unwrap();
    for selector in ["marketplace-kind-Skills", "marketplace-global-prompt-open"] {
        let control = app.find_debug_selector(selector).unwrap();
        let text = app.tree().get(control).unwrap().children[0];
        let text_color = |app: &aqua::UiApp| match &app.tree().get(text).unwrap().kind {
            aqua_ui::tree::NodeKind::Text(text) => text.text_style.gpui_source_color,
            aqua_ui::tree::NodeKind::Box(_)
            | aqua_ui::tree::NodeKind::Media(_)
            | aqua_ui::tree::NodeKind::Scroll(_)
            | aqua_ui::tree::NodeKind::Canvas(_)
            | aqua_ui::tree::NodeKind::Viewport3D(_) => panic!("control label is not text"),
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
    assert_eq!(controller.page.selected_index(), Some(0));
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
    assert_eq!(controller.page.selected_index(), Some(1));
    assert!(controller.selected_entry_key().is_some());
    app.handle_input(&key_event(NamedKey::Escape));
    for action in events.borrow_mut().drain(..) {
        view::dispatch(&mut controller, &mut core, action);
    }
    assert!(controller.page.query.is_empty());
    assert_eq!(controller.page.selected_index(), Some(0));
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

fn discovered(id: &str) -> Item {
    Item::Skill(presentation::SkillRow {
        id: id.into(),
        name: id.into(),
        description: String::new(),
        size: String::new(),
        enabled: false,
        origin: artisan_api::SkillOrigin::Discovered,
        marks: Vec::new(),
        attention: None,
        found_in: None,
    })
}

#[test]
fn enter_installs_the_skill_that_is_painted_selected_after_the_catalog_shrinks() {
    use editor_aqua::marketplace::{Action, Entry, ListKey, Operation, entries};

    let mut page = Marketplace::default();
    page.set_shown(true);
    page.sync_host(Some(HostId("host".into())), &[], &[]);
    page.items = vec![
        discovered("pdf-a"),
        discovered("pdf-b"),
        discovered("pdf-c"),
    ];
    let now = Instant::now();
    page.set_query("pdf".into(), now);
    assert!(page.list_key(ListKey::Down, now).is_none());
    assert!(page.list_key(ListKey::Down, now).is_none());
    page.items.remove(1);
    let painted = entries(&page).into_iter().find_map(|entry| match entry {
        Entry::Item {
            item,
            selected: true,
            ..
        } => Some(item.name().to_owned()),
        Entry::Hero
        | Entry::Notices
        | Entry::Found(_)
        | Entry::Group(_)
        | Entry::SearchFailed(_)
        | Entry::Empty(_)
        | Entry::Item { .. } => None,
    });
    assert_eq!(painted.as_deref(), Some("pdf-c"));
    assert!(matches!(
        page.list_key(ListKey::Accept, now),
        Some(Action::Operation(Operation::Skill { id, .. })) if id == "pdf-c"
    ));
}

#[test]
fn an_mcp_server_does_not_hide_a_skills_sh_skill_with_the_same_name() {
    let mut page = Marketplace::default();
    page.set_shown(true);
    page.sync_host(Some(HostId("host".into())), &[], &[]);
    page.items = vec![Item::Server(ServerRow {
        name: "github".into(),
        target: "npx github-mcp".into(),
    })];
    page.set_query("github".into(), Instant::now());
    page.listings = Remote::Found(vec![listing("listing", "github", 5)]);
    page.set_kind(presentation::Kind::Skills);
    assert!(
        page.results()
            .iter()
            .any(|item| matches!(item, Item::Listing(row) if row.name == "github"))
    );
}

#[test]
fn a_search_started_on_one_host_is_not_shown_after_switching_to_another() {
    use editor_aqua::core::{Core, Screen};
    use editor_aqua::marketplace::controller::MarketplaceController;
    use test_support::editor::core::{Delivered, stub_host};

    let directory = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(&directory.path().join("editor"));
    let first_forge = stub_forge();
    let second_forge = stub_forge();
    let first = stub_host("first", "first", &first_forge);
    let second = stub_host("second", "second", &second_forge);
    connect_live(&mut core, &first);
    connect_live(&mut core, &second);
    let show_on = |core: &mut Core, host: &HostId| {
        core.navigate(Screen::NewThread {
            host: host.clone(),
            project: None,
        });
        core.navigate(Screen::Marketplace);
    };
    show_on(&mut core, &first.id);
    let mut controller = MarketplaceController::default();
    controller.sync(&mut core);
    let delivered = Delivered::watch(controller.wake_mailbox());
    controller.search.state().borrow_mut().set_text("pdf");
    controller.edited(&core);
    first_forge
        .take("GET", "/marketplace/search")
        .respond(vec![listing("first", "pdf", 1)]);
    delivered.wait("the first host's search results");

    show_on(&mut core, &second.id);
    controller.sync(&mut core);
    assert_eq!(controller.page.host.as_ref(), Some(&second.id));
    let on_second = second_forge.take("GET", "/marketplace/search");
    assert!(
        on_second.query().is_some_and(|query| query.contains("pdf")),
        "the search restarts on the second host: {on_second:?}"
    );

    controller.drain(&mut core);
    assert_eq!(
        controller.page.listings,
        Remote::Searching,
        "the first host's results must not appear on the second host"
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
    test_support::wait::wait_until("the stub Forge to go live", Duration::from_secs(10), || {
        core.drain_with(|_, _| {});
        core.host(&config.id)
            .is_some_and(editor_aqua::core::HostData::is_live)
    });
}

fn marketplace_host(
    core: &mut editor_aqua::core::Core,
) -> (
    editor_aqua::marketplace::controller::MarketplaceController,
    test_support::editor::forge::StubForge,
) {
    use editor_aqua::core::Screen;
    use editor_aqua::marketplace::controller::MarketplaceController;
    let forge = stub_forge();
    connect_live(
        core,
        &test_support::editor::core::stub_host("state-host", "Forge", &forge),
    );
    core.navigate(Screen::Marketplace);
    let mut controller = MarketplaceController::default();
    controller.sync(core);
    (controller, forge)
}

fn marketplace_app(
    controller: &editor_aqua::marketplace::controller::MarketplaceController,
    core: &editor_aqua::core::Core,
    emit: std::rc::Rc<dyn Fn(editor_aqua::marketplace::view::ViewAction)>,
) -> aqua::UiApp {
    use aqua_text::{SharedFonts, SharedRuns};
    use editor_aqua::marketplace::view;
    use editor_aqua::ui::theme::ArtisanTheme;
    let fonts = SharedFonts::new();
    editor_aqua::ui::fonts::register_bundled_fonts(&fonts);
    let mut app = aqua::UiApp::new(fonts, SharedRuns::new());
    let element = view::build(controller, core, &ArtisanTheme::resolved(), emit);
    app.mount(|_| element).expect("mount the marketplace");
    app.set_viewport(1000.0, 1600.0, 1.0);
    app.build_frame().expect("lay out the marketplace");
    app
}

#[test]
fn a_failed_skills_sh_search_says_why_and_tries_the_same_query_again() {
    use axum::http::StatusCode;
    use editor_aqua::marketplace::view::{self, ViewAction};
    use std::{cell::RefCell, rc::Rc};
    let directory = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(&directory.path().join("editor"));
    let (mut controller, forge) = marketplace_host(&mut core);
    controller.search.state().borrow_mut().set_text("pdf");
    controller.edited(&core);
    forge.take("GET", "/marketplace/search").fail(
        StatusCode::BAD_GATEWAY,
        "internal",
        "skills.sh did not answer",
    );
    test_support::wait::wait_until(
        "the failed search to reach the page",
        Duration::from_secs(10),
        || {
            controller.drain(&mut core);
            matches!(controller.page.listings, Remote::Failed(_))
        },
    );
    let actions = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&actions);
    let mut app = marketplace_app(
        &controller,
        &core,
        Rc::new(move |action| sink.borrow_mut().push(action)),
    );
    let texts = test_support::editor::ui::painted_texts(&app);
    assert!(
        texts
            .iter()
            .any(|text| text.starts_with("Couldn't search skills.sh.")),
        "{texts:?}"
    );
    assert!(
        !texts
            .iter()
            .any(|text| text.contains("did not answer") || text.contains("/marketplace/search")),
        "the cause stays in the details, off the page: {texts:?}"
    );
    test_support::editor::ui::click(&mut app, "marketplace-search-retry");
    let retry = actions.borrow_mut().pop().unwrap();
    assert!(matches!(retry, ViewAction::RetrySearch));
    assert!(view::dispatch(&mut controller, &mut core, retry));
    assert_eq!(controller.page.listings, Remote::Searching);
    assert_eq!(controller.page.query, "pdf");
    let retried = forge.take("GET", "/marketplace/search");
    assert!(
        retried.query().is_some_and(|query| query.contains("pdf")),
        "Try again searches the same query: {retried:?}"
    );
}

#[test]
fn a_quarantined_skill_being_installed_says_so_on_its_row() {
    use artisan_api::SkillAction;
    use editor_aqua::marketplace::Operation;
    use std::rc::Rc;
    let directory = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(&directory.path().join("editor"));
    let (mut controller, _forge) = marketplace_host(&mut core);
    controller.page.items = vec![discovered("pdf-tools"), discovered("csv-tools")];
    controller.page.found_open = true;
    controller
        .page
        .begin_operation(Operation::Skill {
            id: "pdf-tools".into(),
            action: SkillAction::Adopt,
        })
        .unwrap();
    let texts = test_support::editor::ui::painted_texts(&marketplace_app(
        &controller,
        &core,
        Rc::new(|_| {}),
    ));
    assert_eq!(
        texts.iter().filter(|text| *text == "Installing…").count(),
        1,
        "{texts:?}"
    );
}

#[test]
fn a_long_name_is_truncated_so_its_row_keeps_its_controls() {
    use std::rc::Rc;
    let directory = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(&directory.path().join("editor"));
    let (mut controller, _forge) = marketplace_host(&mut core);
    let long = "a-very-long-skill-name-that-keeps-going-".repeat(6);
    controller.page.items = vec![Item::Server(ServerRow {
        name: long.clone(),
        target: "npx server".into(),
    })];
    let app = marketplace_app(&controller, &core, Rc::new(|_| {}));
    let edit =
        test_support::editor::ui::node_with_key(&app, &format!("marketplace-server-{long}-edit"));
    let bounds = app.tree().rect(edit).unwrap();
    assert!(
        bounds.x + bounds.w <= 1000.0,
        "the row's Edit button stays inside the window at {bounds:?}"
    );
    let name = app
        .tree()
        .traverse()
        .into_iter()
        .find(|node| {
            app.tree().get(*node).is_some_and(|record| {
                matches!(&record.kind, aqua_ui::tree::NodeKind::Text(text) if text.text() == long)
            })
        })
        .expect("the name is painted");
    let painted = app.tree().rect(name).unwrap();
    assert!(
        painted.x + painted.w <= bounds.x,
        "the name ends before the Edit button: {painted:?} vs {bounds:?}"
    );
}
