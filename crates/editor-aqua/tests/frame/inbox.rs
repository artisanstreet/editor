use editor_aqua::frame::inbox::history_height;
use test_support::editor::core::StubbedHost;
use test_support::editor::forge::{TlsServer, empty_state};

#[test]
fn open_history_fills_the_space_the_inbox_leaves_without_overflowing() {
    let open = history_height(800.0, 250.0, 900.0, 1.0);
    assert!(open > 0.0, "history gets the free space");
    assert!(open + 250.0 <= 800.0, "inbox and history fit the viewport");
    test_support::assert_near!(
        history_height(800.0, 250.0, 200.0, 1.0),
        200.0,
        "a short history is only as tall as its rows"
    );
}

#[test]
fn a_crowded_inbox_still_leaves_history_visible_room() {
    let crowded = history_height(800.0, 1000.0, 900.0, 1.0);
    assert!(crowded > 0.0 && crowded < 800.0, "got {crowded}");
    test_support::assert_near!(
        history_height(30.0, 1000.0, 900.0, 1.0),
        0.0,
        "no room, no history"
    );
}

#[test]
fn the_open_fraction_scales_the_height_and_clamps() {
    let open = history_height(800.0, 250.0, 900.0, 1.0);
    test_support::assert_near!(history_height(800.0, 250.0, 900.0, 0.5), open / 2.0);
    test_support::assert_near!(history_height(800.0, 250.0, 900.0, 0.0), 0.0);
    test_support::assert_near!(history_height(800.0, 250.0, 900.0, 2.0), open);
    test_support::assert_near!(history_height(800.0, 250.0, 900.0, -1.0), 0.0);
}

fn live_forge() -> StubbedHost {
    let mut forge = StubbedHost::start(empty_state(), axum::Router::new());
    let host = forge.host.clone();
    forge.core.navigate(editor_aqua::core::Screen::NewThread {
        host,
        project: None,
    });
    forge
}

struct ForgeBeforeState {
    _server: TlsServer,
    release: tokio::sync::watch::Sender<bool>,
    core: editor_aqua::core::Core,
    host: editor_aqua::core::HostId,
    _directory: tempfile::TempDir,
}

fn forge_before_its_state() -> ForgeBeforeState {
    use axum::routing::get;
    use editor_aqua::core::{HostConfig, HostId, HostKind, Screen};
    let (release, released) = tokio::sync::watch::channel(false);
    let router = axum::Router::new()
        .route(
            "/state",
            get(move || {
                let mut released = released.clone();
                async move {
                    drop(released.wait_for(|released| *released).await);
                    axum::Json(empty_state())
                }
            }),
        )
        .route(
            "/events",
            get(|| async {
                test_support::http::event_stream([], test_support::http::StreamEnd::Hang)
            }),
        );
    let server = editor_aqua::core::runtime::runtime().block_on(TlsServer::serve(router));
    let directory = tempfile::tempdir().expect("temporary directory");
    let mut core = test_support::editor::core::test_core(&directory.path().join("editor"));
    let config = HostConfig {
        id: HostId("inbox-forge".into()),
        name: "Inbox".into(),
        address: server.address().to_owned(),
        fingerprint: server.fingerprint().to_owned(),
        kind: HostKind::Remote,
    };
    test_support::editor::core::connect(&mut core, &config);
    core.navigate(Screen::NewThread {
        host: config.id.clone(),
        project: None,
    });
    ForgeBeforeState {
        _server: server,
        release,
        core,
        host: config.id,
        _directory: directory,
    }
}

impl ForgeBeforeState {
    fn answer_with_an_empty_forge(&mut self) {
        self.release.send_replace(true);
        let host = self.host.clone();
        test_support::wait::wait_until(
            "the empty Forge goes live",
            std::time::Duration::from_secs(10),
            || {
                self.core.drain_with(|_, _| {});
                let data = self.core.host(&host).expect("host");
                data.is_live() && data.thread_page(false).is_loaded()
            },
        );
    }
}

type Actions = std::rc::Rc<std::cell::RefCell<Vec<editor_aqua::sidebar::view::ViewAction>>>;

fn rendered(
    sidebar: &editor_aqua::sidebar::controller::Sidebar,
    core: &editor_aqua::core::Core,
) -> (aqua::UiApp, Vec<String>, Actions) {
    use std::rc::Rc;
    let actions: Actions = Rc::default();
    let sink = Rc::clone(&actions);
    let emit: Rc<dyn Fn(editor_aqua::sidebar::view::ViewAction)> =
        Rc::new(move |action| sink.borrow_mut().push(action));
    let states = editor_aqua::frame::inbox::states(
        sidebar,
        core,
        &editor_aqua::ui::theme::ArtisanTheme::resolved(),
        &emit,
    );
    let app = test_support::editor::ui::mounted(
        280.0,
        600.0,
        aqua_ui::element::Box::new()
            .layout("column")
            .width(280.0)
            .children(states)
            .build(),
    );
    let texts = test_support::editor::ui::painted_texts(&app);
    (app, texts, actions)
}

#[test]
fn a_failed_message_search_says_so_and_retries() {
    use axum::http::StatusCode;
    use editor_aqua::sidebar::{
        controller::{Sidebar, TextSearchStatus},
        view::{ViewAction, dispatch},
    };
    let mut forge = live_forge();
    let mut sidebar = Sidebar::new(false);
    sidebar.sync(&forge.core, 0);
    sidebar.search.borrow_mut().set_text("needle");
    sidebar.search_text(&forge.core);
    assert_eq!(sidebar.text_search_status(), TextSearchStatus::Searching);
    forge.take("GET", "/search/text").fail(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal",
        "hits point at known items",
    );
    test_support::wait::wait_until(
        "the failed search reaches the sidebar",
        std::time::Duration::from_secs(10),
        || {
            sidebar.drain(&mut forge.core, 0);
            sidebar.text_search_status() != TextSearchStatus::Searching
        },
    );
    let (mut app, texts, actions) = rendered(&sidebar, &forge.core);
    assert!(
        texts
            .iter()
            .any(|text| text.starts_with("Couldn't search messages: ")),
        "the failure is not shown as an empty result: {texts:?}"
    );
    assert!(
        !texts
            .iter()
            .any(|text| text.starts_with("No threads match"))
    );
    test_support::editor::ui::click(&mut app, "sidebar-search-retry");
    let retry = actions.borrow_mut().pop().expect("Retry emits an action");
    assert!(matches!(retry, ViewAction::Search));
    dispatch(&mut sidebar, &mut forge.core, retry);
    assert_eq!(sidebar.text_search_status(), TextSearchStatus::Searching);
}

#[test]
fn an_empty_inbox_moves_from_loading_to_a_new_thread_prompt() {
    use editor_aqua::sidebar::{
        controller::Sidebar,
        view::{ViewAction, dispatch},
    };
    let mut forge = forge_before_its_state();
    let mut sidebar = Sidebar::new(false);
    sidebar.sync(&forge.core, 0);
    let (_app, texts, _) = rendered(&sidebar, &forge.core);
    assert!(
        texts.is_empty(),
        "nothing claims the inbox is empty before Forge's state arrives: {texts:?}"
    );

    forge.answer_with_an_empty_forge();
    sidebar.sync(&forge.core, 0);
    let (mut app, texts, actions) = rendered(&sidebar, &forge.core);
    assert_eq!(texts, ["No threads yet.", "New thread"]);
    test_support::editor::ui::click(&mut app, "sidebar-empty-new-thread");
    assert!(matches!(
        actions.borrow().as_slice(),
        [ViewAction::NewThread(None)]
    ));

    sidebar.search.borrow_mut().set_text("zz");
    sidebar.sync(&forge.core, 0);
    let (mut app, texts, actions) = rendered(&sidebar, &forge.core);
    assert_eq!(texts, ["No threads match “zz”.", "Clear search"]);
    test_support::editor::ui::click(&mut app, "sidebar-search-clear");
    let clear = actions
        .borrow_mut()
        .pop()
        .expect("Clear search emits an action");
    dispatch(&mut sidebar, &mut forge.core, clear);
    assert_eq!(sidebar.search.borrow().text(), "");
    let (_app, texts, _) = rendered(&sidebar, &forge.core);
    assert_eq!(texts, ["No threads yet.", "New thread"]);
}
