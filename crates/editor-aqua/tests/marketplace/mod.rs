use std::time::{Duration, Instant};

use artisan_api::Listing;
use editor_aqua::core::HostId;
use editor_aqua::core::client::{CallError, CallFailure};
use editor_aqua::marketplace::presentation::{self, Item, ServerRow};
use editor_aqua::marketplace::{Marketplace, OperationError, Remote, Settled};

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
    page.sync_host(Some(HostId("host".into())), &[], None, &[]);
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
fn a_change_on_one_host_finishes_there_after_switching_to_another() {
    use artisan_api::SkillAction;
    use editor_aqua::marketplace::Operation;

    let first = HostId("first".into());
    let mut page = Marketplace::default();
    page.set_shown(true);
    page.sync_host(Some(first.clone()), &[], None, &[]);
    let request = page
        .begin_operation(Operation::Skill {
            id: "skill".into(),
            action: SkillAction::Disable,
        })
        .unwrap();
    assert!(
        page.begin_operation(Operation::Skill {
            id: "skill".into(),
            action: SkillAction::Enable,
        })
        .is_none(),
        "a skill already being changed takes no second change"
    );
    page.sync_host(Some(HostId("second".into())), &[], None, &[]);
    assert_eq!(
        page.pending().count(),
        0,
        "the second host has nothing saving"
    );
    assert!(matches!(
        page.finish_operation(
            &request,
            Err(refused()),
            editor_aqua::core::CatalogRead::default()
        ),
        Settled::Done { .. }
    ));
    assert_eq!(page.notices().count(), 0);
    page.sync_host(Some(first), &[], None, &[]);
    assert_eq!(
        page.notices().count(),
        1,
        "the first host's failure shows once it is back on screen"
    );
}

#[test]
fn a_replaced_host_rejects_old_requests_and_names_each_change_it_may_have_lost() {
    use artisan_api::SkillAction;
    use editor_aqua::marketplace::Operation;

    let host = HostId("same-host".into());
    let mut page = Marketplace::default();
    page.set_shown(true);
    page.sync_host(Some(host.clone()), &[], None, &[]);
    let operation = page
        .begin_operation(Operation::Skill {
            id: "alpha".into(),
            action: SkillAction::Disable,
        })
        .unwrap();
    assert!(
        page.begin_operation(Operation::Skill {
            id: "beta".into(),
            action: SkillAction::Disable,
        })
        .is_none(),
        "the second toggle waits for the first"
    );
    page.replace_host(&host);
    assert!(matches!(
        page.finish_operation(
            &operation,
            Err(refused()),
            editor_aqua::core::CatalogRead::default()
        ),
        Settled::Stale
    ));
    assert_eq!(page.pending().count(), 0);
    assert_eq!(
        page.notices()
            .map(editor_aqua::core::Problem::headline)
            .collect::<Vec<_>>(),
        [
            "The change to alpha may not have been saved: the Forge connection was replaced while it was sent.",
            "Couldn't disable beta: the Forge connection was replaced before the change was sent."
        ],
        "the toggle in flight may be lost and the queued one never reached Forge"
    );
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
    operate(
        &mut controller,
        &mut core,
        Operation::Skill {
            id: "skill".into(),
            action: SkillAction::Adopt,
        },
    );
    assert_eq!(controller.page.pending().count(), 1);
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
    assert_eq!(
        controller.page.pending().count(),
        0,
        "the replaced host's install is not shown as saving on the new one"
    );
    assert_eq!(
        controller
            .page
            .notices()
            .map(editor_aqua::core::Problem::headline)
            .collect::<Vec<_>>(),
        [
            "The change to skill may not have been saved: the Forge connection was replaced while it was sent."
        ],
        "the install that was in flight when the host was replaced says it may be lost"
    );
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
    page.sync_host(Some(HostId("host".into())), &[], None, &[]);
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
    page.sync_host(Some(HostId("host".into())), &[], None, &[]);
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
fn a_long_name_is_truncated_so_its_row_keeps_its_controls_and_shows_in_full_on_hover() {
    use std::rc::Rc;
    let directory = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(&directory.path().join("editor"));
    let (mut controller, _forge) = marketplace_host(&mut core);
    let long = "a-very-long-skill-name-that-keeps-going-".repeat(6);
    controller.page.items = vec![Item::Server(ServerRow {
        name: long.clone(),
        target: "npx server".into(),
    })];
    let mut app = marketplace_app(&controller, &core, Rc::new(|_| {}));
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
    let full_names = |app: &aqua::UiApp| {
        app.tree()
            .traverse()
            .into_iter()
            .filter(|node| {
                app.tree().get(*node).is_some_and(|record| {
                    matches!(&record.kind, aqua_ui::tree::NodeKind::Text(text) if text.text() == long)
                })
            })
            .count()
    };
    assert_eq!(full_names(&app), 1, "no tooltip before the pointer rests");
    app.handle_input(&aqua_core::InputEvent::PointerMove(
        aqua_core::PointerEvent {
            contact: 0,
            kind: aqua_core::PointerKind::Mouse,
            position: aqua_core::Point::new(
                painted.w.mul_add(0.5, painted.x),
                painted.h.mul_add(0.5, painted.y),
            ),
            buttons: 0,
        },
    ));
    for frame in 1..=60_u32 {
        app.advance_clock(f64::from(frame) * 0.016);
        app.build_frame().unwrap();
    }
    assert_eq!(
        full_names(&app),
        2,
        "resting on the cut-off name shows the whole name in a tooltip"
    );
}

#[test]
fn arrow_keys_move_focus_and_the_kind_through_the_kind_tabs() {
    use aqua_core::{InputEvent, Key, KeyEvent, Modifiers, NamedKey, Role};
    use editor_aqua::marketplace::presentation::Kind;
    use editor_aqua::marketplace::view::ViewAction;
    use std::{cell::RefCell, rc::Rc};
    let directory = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(directory.path());
    let (controller, _forge) = marketplace_host(&mut core);
    let chosen = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&chosen);
    let mut app = marketplace_app(
        &controller,
        &core,
        Rc::new(move |action| {
            if let ViewAction::SetKind(kind) = action {
                sink.borrow_mut().push(kind);
            }
        }),
    );
    let semantics = app.tree().emit_semantics();
    let tabs: Vec<_> = semantics
        .iter()
        .filter(|node| node.role == Role::Tab)
        .collect();
    assert_eq!(tabs.len(), 3, "All, Skills and MCP servers are tabs");
    assert!(
        semantics.iter().any(|node| node.role == Role::TabList),
        "the kinds are a tab list"
    );
    let all = tabs[0].node;
    assert!(app.tree_mut().set_focused(Some(all)));
    let mut focused = Vec::new();
    for key in [
        NamedKey::Right,
        NamedKey::End,
        NamedKey::Home,
        NamedKey::Left,
    ] {
        let event = KeyEvent {
            key: Key::Named(key),
            modifiers: Modifiers::NONE,
            repeat: false,
        };
        app.handle_input(&InputEvent::KeyDown(event));
        app.handle_input(&InputEvent::KeyUp(event));
        focused.extend(
            app.tree()
                .emit_semantics()
                .into_iter()
                .find(|node| node.focused)
                .map(|node| node.label),
        );
    }
    assert_eq!(
        *chosen.borrow(),
        [Kind::Skills, Kind::Servers, Kind::All, Kind::Servers]
    );
    assert_eq!(
        focused,
        ["Skills 0", "MCP servers 0", "All", "MCP servers 0"]
    );
}

fn user_skill(id: &str, enabled: bool) -> artisan_api::CatalogSkill {
    artisan_api::CatalogSkill {
        id: id.into(),
        name: id.into(),
        description: String::new(),
        origin: artisan_api::SkillOrigin::User,
        enabled,
        file_count: 1,
        byte_count: 10,
        engines: Vec::new(),
    }
}

fn shown_enabled(
    controller: &editor_aqua::marketplace::controller::MarketplaceController,
) -> Vec<(String, bool)> {
    controller
        .page
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Skill(row) => Some((row.id.clone(), row.enabled)),
            Item::Server(_) | Item::Listing(_) => None,
        })
        .collect()
}

type Controller = editor_aqua::marketplace::controller::MarketplaceController;

fn pump(controller: &mut Controller, core: &mut editor_aqua::core::Core) {
    core.drain_with(|core, event| controller.core_event(core, &event));
    controller.drain(core);
}

fn operate(
    controller: &mut Controller,
    core: &mut editor_aqua::core::Core,
    operation: editor_aqua::marketplace::Operation,
) {
    use editor_aqua::marketplace::view::{self, ViewAction};
    view::dispatch(controller, core, ViewAction::Operate(operation));
}

fn disable(id: &str) -> editor_aqua::marketplace::Operation {
    editor_aqua::marketplace::Operation::Skill {
        id: id.into(),
        action: artisan_api::SkillAction::Disable,
    }
}

fn next_request(
    controller: &mut Controller,
    core: &mut editor_aqua::core::Core,
    forge: &test_support::editor::forge::StubForge,
    method: &str,
    path: &str,
) -> test_support::editor::forge::HeldRequest {
    test_support::wait::wait_for(&format!("{method} {path}"), Duration::from_secs(10), || {
        pump(controller, core);
        forge.try_take(method, path)
    })
}

fn no_catalog_read(controller: &mut Controller, core: &mut editor_aqua::core::Core) {
    pump(controller, core);
    assert!(
        !core
            .active_host()
            .is_some_and(|host| host.catalog_load().loading),
        "the Editor started reading the catalog"
    );
}

fn read_landed(
    controller: &mut Controller,
    core: &mut editor_aqua::core::Core,
    before: Option<editor_aqua::core::CatalogRead>,
) {
    test_support::wait::wait_until(
        "a newer catalog read to land",
        Duration::from_secs(10),
        || {
            pump(controller, core);
            core.active_host()
                .is_some_and(|host| host.fresh_catalog_read() > before)
        },
    );
}

fn fresh_read(core: &editor_aqua::core::Core) -> Option<editor_aqua::core::CatalogRead> {
    core.active_host()
        .and_then(editor_aqua::core::HostData::fresh_catalog_read)
}

fn notice_applied(
    controller: &mut Controller,
    core: &mut editor_aqua::core::Core,
    forge: &test_support::editor::forge::StubForge,
    disabled: &[&str],
) {
    let value = serde_json::json!(disabled);
    forge.publish(artisan_api::Event::Setting {
        key: "disabled_skills".into(),
        value: value.clone(),
    });
    test_support::wait::wait_until(
        "Forge's change notice to apply",
        Duration::from_secs(10),
        || {
            pump(controller, core);
            core.active_host()
                .and_then(|host| host.setting("disabled_skills"))
                == Some(&value)
        },
    );
}

fn settled(controller: &mut Controller, core: &mut editor_aqua::core::Core, saving: usize) {
    test_support::wait::wait_until(
        &format!("{saving} saves left"),
        Duration::from_secs(10),
        || {
            pump(controller, core);
            controller.page.pending().count() == saving
        },
    );
}

fn loaded_catalog(
    core: &mut editor_aqua::core::Core,
    skills: Vec<artisan_api::CatalogSkill>,
) -> (Controller, test_support::editor::forge::StubForge) {
    let (mut controller, forge) = marketplace_host(core);
    let count = skills.len();
    forge.take("GET", "/skills/catalog").respond(skills);
    test_support::wait::wait_until("the catalog to show", Duration::from_secs(10), || {
        pump(&mut controller, core);
        controller.page.items.len() == count
    });
    (controller, forge)
}

fn switches(ids: &[&str], on: &[&str]) -> Vec<(String, bool)> {
    ids.iter()
        .map(|id| ((*id).to_owned(), on.contains(id)))
        .collect()
}

fn toggled_alpha_then_beta() -> (
    tempfile::TempDir,
    editor_aqua::core::Core,
    Controller,
    test_support::editor::forge::StubForge,
    test_support::editor::forge::HeldRequest,
) {
    let directory = tempfile::tempdir().expect("a test data folder");
    let mut core = test_support::editor::core::test_core(&directory.path().join("editor"));
    let (mut controller, forge) = loaded_catalog(
        &mut core,
        vec![user_skill("alpha", true), user_skill("beta", true)],
    );
    operate(&mut controller, &mut core, disable("alpha"));
    operate(&mut controller, &mut core, disable("beta"));
    assert_eq!(
        controller.page.pending().count(),
        2,
        "both toggles are saving"
    );
    assert_eq!(
        shown_enabled(&controller),
        switches(&["alpha", "beta"], &[]),
        "both switches show the choice before Forge answers"
    );
    let alpha = next_request(
        &mut controller,
        &mut core,
        &forge,
        "POST",
        "/skills/alpha/disable",
    );
    pump(&mut controller, &mut core);
    assert!(
        forge.try_take("POST", "/skills/beta/disable").is_none(),
        "beta waits for alpha's save"
    );
    (directory, core, controller, forge, alpha)
}

#[test]
fn a_toggle_answered_after_its_change_notice_holds_until_a_read_issued_after_the_answer() {
    let (_directory, mut core, mut controller, forge, alpha) = toggled_alpha_then_beta();
    let both_off = switches(&["alpha", "beta"], &[]);

    notice_applied(&mut controller, &mut core, &forge, &["alpha"]);
    let early = next_request(&mut controller, &mut core, &forge, "GET", "/skills/catalog");
    assert_eq!(
        shown_enabled(&controller),
        both_off,
        "Forge's change notice keeps the list and the choices on screen while it reloads"
    );
    alpha.respond(vec![user_skill("alpha", false), user_skill("beta", true)]);
    let beta = next_request(
        &mut controller,
        &mut core,
        &forge,
        "POST",
        "/skills/beta/disable",
    );
    let reread = next_request(&mut controller, &mut core, &forge, "GET", "/skills/catalog");
    early.respond(vec![user_skill("alpha", true), user_skill("beta", true)]);
    pump(&mut controller, &mut core);
    assert_eq!(
        shown_enabled(&controller),
        both_off,
        "a read issued before alpha's answer cannot turn alpha back on"
    );
    reread.respond(vec![user_skill("alpha", false), user_skill("beta", true)]);
    read_landed(&mut controller, &mut core, None);
    assert_eq!(shown_enabled(&controller), both_off);
    no_catalog_read(&mut controller, &mut core);

    let before = fresh_read(&core);
    beta.respond(vec![user_skill("alpha", false), user_skill("beta", false)]);
    settled(&mut controller, &mut core, 0);
    next_request(&mut controller, &mut core, &forge, "GET", "/skills/catalog")
        .respond(vec![user_skill("alpha", false), user_skill("beta", false)]);
    read_landed(&mut controller, &mut core, before);
    assert_eq!(
        shown_enabled(&controller),
        both_off,
        "beta's answer, older than the read alpha's answer started, waits for one more read"
    );
    no_catalog_read(&mut controller, &mut core);
}

#[test]
fn a_later_toggle_answered_before_its_notice_holds_through_the_earlier_toggles_read() {
    let (_directory, mut core, mut controller, forge, alpha) = toggled_alpha_then_beta();
    let both_off = switches(&["alpha", "beta"], &[]);

    alpha.respond(vec![user_skill("alpha", false), user_skill("beta", true)]);
    let beta = next_request(
        &mut controller,
        &mut core,
        &forge,
        "POST",
        "/skills/beta/disable",
    );
    notice_applied(&mut controller, &mut core, &forge, &["alpha"]);
    let alphas_read = next_request(&mut controller, &mut core, &forge, "GET", "/skills/catalog");
    beta.respond(vec![user_skill("alpha", false), user_skill("beta", false)]);
    let reread = next_request(&mut controller, &mut core, &forge, "GET", "/skills/catalog");
    alphas_read.respond(vec![user_skill("alpha", false), user_skill("beta", true)]);
    pump(&mut controller, &mut core);
    assert_eq!(
        shown_enabled(&controller),
        both_off,
        "the read alpha's notice started, before beta was saved, cannot turn beta back on"
    );
    reread.respond(vec![user_skill("alpha", false), user_skill("beta", false)]);
    read_landed(&mut controller, &mut core, None);
    assert_eq!(shown_enabled(&controller), both_off);

    let before = fresh_read(&core);
    notice_applied(&mut controller, &mut core, &forge, &["alpha", "beta"]);
    next_request(&mut controller, &mut core, &forge, "GET", "/skills/catalog")
        .respond(vec![user_skill("alpha", false), user_skill("beta", false)]);
    read_landed(&mut controller, &mut core, before);
    assert_eq!(shown_enabled(&controller), both_off);
}

#[test]
fn a_saved_toggle_keeps_its_value_when_both_rereads_fail_until_the_user_tries_again() {
    use axum::http::StatusCode;
    use editor_aqua::marketplace::view::{self, ViewAction};
    let (_directory, mut core, mut controller, forge, alpha) = toggled_alpha_then_beta();
    let both_off = switches(&["alpha", "beta"], &[]);

    notice_applied(&mut controller, &mut core, &forge, &["alpha"]);
    let early = next_request(&mut controller, &mut core, &forge, "GET", "/skills/catalog");
    alpha.respond(vec![user_skill("alpha", false), user_skill("beta", true)]);
    let _beta = next_request(
        &mut controller,
        &mut core,
        &forge,
        "POST",
        "/skills/beta/disable",
    );
    let reread = next_request(&mut controller, &mut core, &forge, "GET", "/skills/catalog");
    early.fail(StatusCode::SERVICE_UNAVAILABLE, "internal", "busy");
    reread.fail(StatusCode::SERVICE_UNAVAILABLE, "internal", "busy");
    let retry = next_request(&mut controller, &mut core, &forge, "GET", "/skills/catalog");
    assert_eq!(
        shown_enabled(&controller),
        both_off,
        "a failed read keeps alpha's saved choice and reads once more"
    );
    retry.fail(StatusCode::SERVICE_UNAVAILABLE, "internal", "busy");
    test_support::wait::wait_until(
        "the second failure to show",
        Duration::from_secs(10),
        || {
            pump(&mut controller, &mut core);
            core.active_host()
                .is_some_and(|host| host.catalog_load().error.is_some())
        },
    );
    no_catalog_read(&mut controller, &mut core);
    assert_eq!(
        shown_enabled(&controller),
        both_off,
        "after the last automatic read fails, alpha still shows what Forge accepted"
    );
    view::dispatch(&mut controller, &mut core, ViewAction::Retry);
    let manual = next_request(&mut controller, &mut core, &forge, "GET", "/skills/catalog");
    manual.respond(vec![user_skill("alpha", false), user_skill("beta", true)]);
    read_landed(&mut controller, &mut core, None);
    assert_eq!(shown_enabled(&controller), both_off);
}

#[test]
fn a_refused_second_toggle_shows_the_first_one_forge_accepted() {
    use axum::http::StatusCode;
    let directory = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(&directory.path().join("editor"));
    let (mut controller, forge) = loaded_catalog(&mut core, vec![user_skill("alpha", true)]);
    operate(&mut controller, &mut core, disable("alpha"));
    let alpha = next_request(
        &mut controller,
        &mut core,
        &forge,
        "POST",
        "/skills/alpha/disable",
    );
    notice_applied(&mut controller, &mut core, &forge, &["alpha"]);
    let early = next_request(&mut controller, &mut core, &forge, "GET", "/skills/catalog");
    alpha.respond(vec![user_skill("alpha", false)]);
    let reread = next_request(&mut controller, &mut core, &forge, "GET", "/skills/catalog");
    settled(&mut controller, &mut core, 0);
    assert_eq!(shown_enabled(&controller), switches(&["alpha"], &[]));

    operate(
        &mut controller,
        &mut core,
        editor_aqua::marketplace::Operation::Skill {
            id: "alpha".into(),
            action: artisan_api::SkillAction::Enable,
        },
    );
    assert_eq!(shown_enabled(&controller), switches(&["alpha"], &["alpha"]));
    next_request(
        &mut controller,
        &mut core,
        &forge,
        "POST",
        "/skills/alpha/enable",
    )
    .fail(StatusCode::BAD_REQUEST, "bad_request", "not now");
    settled(&mut controller, &mut core, 0);
    assert_eq!(
        shown_enabled(&controller),
        switches(&["alpha"], &[]),
        "the refused enable falls back to the disable Forge accepted, not the stale catalog"
    );
    early.respond(vec![user_skill("alpha", true)]);
    reread.respond(vec![user_skill("alpha", false)]);
    read_landed(&mut controller, &mut core, None);
    assert_eq!(shown_enabled(&controller), switches(&["alpha"], &[]));
}

#[test]
fn an_install_reply_read_before_a_newer_catalog_is_not_adopted() {
    use editor_aqua::marketplace::Operation;
    use editor_aqua::marketplace::view::{self, ViewAction};
    let directory = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(&directory.path().join("editor"));
    let (mut controller, forge) = loaded_catalog(&mut core, vec![user_skill("alpha", true)]);
    operate(&mut controller, &mut core, disable("alpha"));
    operate(
        &mut controller,
        &mut core,
        Operation::Listing("pdf-tools".into()),
    );
    let alpha = next_request(
        &mut controller,
        &mut core,
        &forge,
        "POST",
        "/skills/alpha/disable",
    );
    let install = next_request(
        &mut controller,
        &mut core,
        &forge,
        "POST",
        "/marketplace/install",
    );
    alpha.respond(vec![user_skill("alpha", false)]);
    settled(&mut controller, &mut core, 1);
    let before = fresh_read(&core);
    view::dispatch(&mut controller, &mut core, ViewAction::Retry);
    next_request(&mut controller, &mut core, &forge, "GET", "/skills/catalog")
        .respond(vec![user_skill("alpha", false)]);
    read_landed(&mut controller, &mut core, before);
    let newer = fresh_read(&core);
    install.respond(vec![user_skill("alpha", true)]);
    settled(&mut controller, &mut core, 0);
    assert_eq!(
        fresh_read(&core),
        None,
        "the install's catalog, read before the newer one, is not adopted and a reread starts"
    );
    assert_eq!(
        shown_enabled(&controller),
        switches(&["alpha"], &[]),
        "the install's older catalog does not replace the newer one that confirmed alpha"
    );
    next_request(&mut controller, &mut core, &forge, "GET", "/skills/catalog")
        .respond(vec![user_skill("alpha", false)]);
    read_landed(&mut controller, &mut core, newer);
    assert_eq!(shown_enabled(&controller), switches(&["alpha"], &[]));
}

#[test]
fn an_install_answered_first_with_an_older_catalog_leaves_the_saving_toggle_shown() {
    use editor_aqua::marketplace::Operation;
    let directory = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(&directory.path().join("editor"));
    let (mut controller, forge) = loaded_catalog(&mut core, vec![user_skill("alpha", true)]);
    operate(&mut controller, &mut core, disable("alpha"));
    operate(
        &mut controller,
        &mut core,
        Operation::Listing("pdf-tools".into()),
    );
    let alpha = next_request(
        &mut controller,
        &mut core,
        &forge,
        "POST",
        "/skills/alpha/disable",
    );
    next_request(
        &mut controller,
        &mut core,
        &forge,
        "POST",
        "/marketplace/install",
    )
    .respond(vec![user_skill("alpha", true)]);
    settled(&mut controller, &mut core, 1);
    assert_eq!(
        shown_enabled(&controller),
        switches(&["alpha"], &[]),
        "the install's catalog does not undo the toggle still saving"
    );
    alpha.respond(vec![user_skill("alpha", false)]);
    settled(&mut controller, &mut core, 0);
    assert_eq!(shown_enabled(&controller), switches(&["alpha"], &[]));
}

#[test]
fn a_removed_host_drops_its_unsaved_changes_instead_of_keeping_notices() {
    use editor_aqua::core::Screen;
    let directory = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(&directory.path().join("editor"));
    let (mut controller, forge) = loaded_catalog(
        &mut core,
        vec![user_skill("alpha", true), user_skill("beta", true)],
    );
    operate(&mut controller, &mut core, disable("alpha"));
    operate(&mut controller, &mut core, disable("beta"));
    let _alpha = next_request(
        &mut controller,
        &mut core,
        &forge,
        "POST",
        "/skills/alpha/disable",
    );
    let config = test_support::editor::core::stub_host("state-host", "Forge", &forge);
    core.remove_host(&config.id).unwrap();
    controller.sync(&mut core);
    connect_live(&mut core, &config);
    core.navigate(Screen::Marketplace);
    controller.sync(&mut core);
    assert_eq!(controller.page.pending().count(), 0);
    assert_eq!(
        controller.page.notices().count(),
        0,
        "a host the user removed brings back no notices about it"
    );
    assert!(
        controller.page.begin_operation(disable("beta")).is_some(),
        "the paired host is the one on screen"
    );
}

#[test]
fn an_overlapping_reply_read_before_a_save_does_not_undo_it() {
    use editor_aqua::marketplace::Operation;
    let directory = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(&directory.path().join("editor"));
    let (mut controller, forge) = loaded_catalog(&mut core, vec![user_skill("alpha", true)]);

    operate(&mut controller, &mut core, disable("alpha"));
    operate(
        &mut controller,
        &mut core,
        Operation::Listing("pdf-tools".into()),
    );
    let alpha = next_request(
        &mut controller,
        &mut core,
        &forge,
        "POST",
        "/skills/alpha/disable",
    );
    let install = next_request(
        &mut controller,
        &mut core,
        &forge,
        "POST",
        "/marketplace/install",
    );
    alpha.respond(vec![user_skill("alpha", false)]);
    settled(&mut controller, &mut core, 1);
    no_catalog_read(&mut controller, &mut core);
    install.respond(vec![user_skill("alpha", true)]);
    settled(&mut controller, &mut core, 0);
    assert_eq!(
        shown_enabled(&controller),
        switches(&["alpha"], &[]),
        "the install's catalog, read before alpha's save, does not turn alpha back on"
    );
    let reload = next_request(&mut controller, &mut core, &forge, "GET", "/skills/catalog");
    reload.respond(vec![user_skill("alpha", false)]);
    test_support::wait::wait_until("the reload to land", Duration::from_secs(10), || {
        pump(&mut controller, &mut core);
        core.active_host()
            .is_some_and(|host| host.fresh_catalog_read().is_some())
    });
    no_catalog_read(&mut controller, &mut core);
    assert_eq!(shown_enabled(&controller), switches(&["alpha"], &[]));
}

#[test]
fn overlapping_saves_reload_the_catalog_once_even_when_the_last_one_fails() {
    use axum::http::StatusCode;
    use editor_aqua::marketplace::Operation;
    let directory = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(&directory.path().join("editor"));
    let (mut controller, forge) = loaded_catalog(&mut core, vec![user_skill("alpha", true)]);

    operate(&mut controller, &mut core, disable("alpha"));
    operate(
        &mut controller,
        &mut core,
        Operation::Listing("pdf-tools".into()),
    );
    let alpha = next_request(
        &mut controller,
        &mut core,
        &forge,
        "POST",
        "/skills/alpha/disable",
    );
    let install = next_request(
        &mut controller,
        &mut core,
        &forge,
        "POST",
        "/marketplace/install",
    );
    alpha.respond(vec![user_skill("alpha", false)]);
    settled(&mut controller, &mut core, 1);
    no_catalog_read(&mut controller, &mut core);
    install.fail(StatusCode::INTERNAL_SERVER_ERROR, "internal", "disk full");
    let reload = next_request(&mut controller, &mut core, &forge, "GET", "/skills/catalog");
    assert_eq!(
        controller.page.notices().count(),
        1,
        "the failed install says so"
    );
    reload.respond(vec![user_skill("alpha", false)]);
    test_support::wait::wait_until("the reload to land", Duration::from_secs(10), || {
        pump(&mut controller, &mut core);
        core.active_host()
            .is_some_and(|host| host.fresh_catalog_read().is_some())
    });
    no_catalog_read(&mut controller, &mut core);
    assert_eq!(shown_enabled(&controller), switches(&["alpha"], &[]));
}

#[test]
fn two_failures_answered_together_both_say_so() {
    use axum::http::StatusCode;
    use editor_aqua::marketplace::Operation;
    let directory = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(&directory.path().join("editor"));
    let (mut controller, forge) = loaded_catalog(&mut core, vec![user_skill("alpha", true)]);

    operate(&mut controller, &mut core, disable("alpha"));
    operate(
        &mut controller,
        &mut core,
        Operation::Listing("pdf-tools".into()),
    );
    let alpha = next_request(
        &mut controller,
        &mut core,
        &forge,
        "POST",
        "/skills/alpha/disable",
    );
    let install = next_request(
        &mut controller,
        &mut core,
        &forge,
        "POST",
        "/marketplace/install",
    );
    alpha.fail(StatusCode::INTERNAL_SERVER_ERROR, "internal", "disk full");
    install.fail(StatusCode::INTERNAL_SERVER_ERROR, "internal", "disk full");
    settled(&mut controller, &mut core, 0);
    let mut notices: Vec<&str> = controller
        .page
        .notices()
        .map(editor_aqua::core::Problem::headline)
        .collect();
    notices.sort_unstable();
    assert_eq!(
        notices,
        ["Couldn't disable alpha.", "Couldn't install pdf-tools."],
        "each failed change keeps its own notice"
    );
    assert_eq!(
        shown_enabled(&controller),
        switches(&["alpha"], &["alpha"]),
        "the refused toggle shows what Forge holds"
    );
}

#[test]
fn a_toggle_waiting_on_one_host_is_still_sent_after_switching_to_another() {
    use editor_aqua::core::{Core, Screen};
    use test_support::editor::core::stub_host;

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
    let mut controller = Controller::default();
    controller.sync(&mut core);
    first_forge
        .take("GET", "/skills/catalog")
        .respond(vec![user_skill("alpha", true), user_skill("beta", true)]);
    test_support::wait::wait_until("the catalog to show", Duration::from_secs(10), || {
        pump(&mut controller, &mut core);
        controller.page.items.len() == 2
    });
    operate(&mut controller, &mut core, disable("alpha"));
    operate(&mut controller, &mut core, disable("beta"));
    let alpha = next_request(
        &mut controller,
        &mut core,
        &first_forge,
        "POST",
        "/skills/alpha/disable",
    );

    show_on(&mut core, &second.id);
    controller.sync(&mut core);
    alpha.respond(vec![user_skill("alpha", false), user_skill("beta", true)]);
    let beta = next_request(
        &mut controller,
        &mut core,
        &first_forge,
        "POST",
        "/skills/beta/disable",
    );
    beta.respond(vec![user_skill("alpha", false), user_skill("beta", false)]);

    show_on(&mut core, &first.id);
    controller.sync(&mut core);
    settled(&mut controller, &mut core, 0);
    assert_eq!(
        shown_enabled(&controller),
        switches(&["alpha", "beta"], &[])
    );
}

#[test]
fn two_refused_toggles_each_name_their_skill() {
    use axum::http::StatusCode;
    let (_directory, mut core, mut controller, forge, alpha) = toggled_alpha_then_beta();
    alpha.fail(StatusCode::BAD_REQUEST, "bad_request", "not now");
    next_request(
        &mut controller,
        &mut core,
        &forge,
        "POST",
        "/skills/beta/disable",
    )
    .fail(StatusCode::BAD_REQUEST, "bad_request", "not now");
    settled(&mut controller, &mut core, 0);
    assert_eq!(
        controller
            .page
            .notices()
            .map(editor_aqua::core::Problem::headline)
            .collect::<Vec<_>>(),
        ["Couldn't disable alpha.", "Couldn't disable beta."]
    );
}

#[test]
fn a_replaced_host_keeps_the_notices_it_already_showed() {
    let host = HostId("same-host".into());
    let mut page = Marketplace::default();
    page.set_shown(true);
    page.sync_host(Some(host.clone()), &[], None, &[]);
    let alpha = page.begin_operation(disable("alpha")).unwrap();
    assert!(matches!(
        page.finish_operation(
            &alpha,
            Err(refused()),
            editor_aqua::core::CatalogRead::default()
        ),
        Settled::Done { .. }
    ));
    let _beta = page.begin_operation(disable("beta")).unwrap();
    page.replace_host(&host);
    assert_eq!(
        page.notices()
            .map(editor_aqua::core::Problem::headline)
            .collect::<Vec<_>>(),
        [
            "Couldn't disable alpha.",
            "The change to beta may not have been saved: the Forge connection was replaced while it was sent."
        ],
        "the refusal shown before the replacement stays beside the change it may have lost"
    );
}

#[test]
fn a_replaced_host_that_is_then_removed_leaves_no_notices_when_paired_again() {
    use editor_aqua::core::Screen;
    let directory = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(&directory.path().join("editor"));
    let (mut controller, forge) = loaded_catalog(&mut core, vec![user_skill("alpha", true)]);
    operate(&mut controller, &mut core, disable("alpha"));
    let _alpha = next_request(
        &mut controller,
        &mut core,
        &forge,
        "POST",
        "/skills/alpha/disable",
    );
    let config = test_support::editor::core::stub_host("state-host", "Forge", &forge);
    core.remove_host(&config.id).unwrap();
    connect_live(&mut core, &config);
    core.navigate(Screen::Marketplace);
    controller.sync(&mut core);
    assert_eq!(
        controller.page.notices().count(),
        1,
        "the replacement says the change in flight may be lost"
    );
    core.remove_host(&config.id).unwrap();
    controller.sync(&mut core);
    connect_live(&mut core, &config);
    core.navigate(Screen::Marketplace);
    controller.sync(&mut core);
    assert_eq!(
        controller.page.notices().count(),
        0,
        "removing the replacement forgets its notices"
    );
    assert!(
        controller.page.begin_operation(disable("alpha")).is_some(),
        "the paired host is the one on screen"
    );
}

#[test]
fn a_failed_skills_sh_install_names_the_listing_as_shown() {
    use editor_aqua::marketplace::Operation;
    let mut page = Marketplace::default();
    page.set_shown(true);
    page.sync_host(Some(HostId("forge".into())), &[], None, &[]);
    let now = Instant::now();
    page.set_query("pdf".into(), now);
    let search = page.take_search(now + Duration::from_millis(250)).unwrap();
    page.finish_search(
        &search,
        Ok(vec![listing("anthropics/skills/pdf", "pdf-tools", 10)]),
    );
    let install = page
        .begin_operation(Operation::Listing("anthropics/skills/pdf".into()))
        .unwrap();
    page.finish_operation(
        &install,
        Err(refused()),
        editor_aqua::core::CatalogRead::default(),
    );
    assert_eq!(
        page.notices()
            .map(editor_aqua::core::Problem::headline)
            .collect::<Vec<_>>(),
        ["Couldn't install pdf-tools."]
    );
}

#[test]
fn a_repeated_answer_never_settles_a_later_change_to_the_same_skill() {
    use editor_aqua::marketplace::{Operation, Reply};
    let mut page = Marketplace::default();
    page.set_shown(true);
    page.sync_host(Some(HostId("forge".into())), &[], None, &[]);
    let first = page.begin_operation(disable("alpha")).unwrap();
    let reads = editor_aqua::core::CatalogRead::default();
    assert!(matches!(
        page.finish_operation(&first, Ok(Reply::Adopted), reads),
        Settled::Done { .. }
    ));
    let _second = page
        .begin_operation(Operation::Skill {
            id: "alpha".into(),
            action: artisan_api::SkillAction::Enable,
        })
        .unwrap();
    assert!(
        matches!(
            page.finish_operation(&first, Ok(Reply::Adopted), reads),
            Settled::Stale
        ),
        "the first send's answer arriving twice is ignored"
    );
    assert_eq!(
        page.pending().count(),
        1,
        "the enable that followed is still saving"
    );
}

#[test]
fn a_reconnect_that_resnapshots_keeps_the_list_on_screen_while_it_reloads() {
    let directory = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(&directory.path().join("editor"));
    let (mut controller, forge) = loaded_catalog(
        &mut core,
        vec![user_skill("alpha", true), user_skill("beta", false)],
    );
    forge.publish(artisan_api::Event::Reset);
    let reload = next_request(&mut controller, &mut core, &forge, "GET", "/skills/catalog");
    assert_eq!(
        shown_enabled(&controller),
        switches(&["alpha", "beta"], &["alpha"]),
        "the skills stay listed while the catalog reloads"
    );
    reload.respond(vec![user_skill("alpha", true), user_skill("beta", false)]);
    read_landed(&mut controller, &mut core, None);
    assert_eq!(
        shown_enabled(&controller),
        switches(&["alpha", "beta"], &["alpha"])
    );
}
