use editor_aqua::frame::inbox::history_height;

#[test]
fn open_history_fills_the_space_the_inbox_leaves_without_overflowing() {
    let open = history_height(800.0, 250.0, 900.0, 1.0);
    assert!(open > 0.0, "history gets the free space");
    assert!(open + 250.0 <= 800.0, "inbox and history fit the viewport");
    assert_eq!(
        history_height(800.0, 250.0, 200.0, 1.0),
        200.0,
        "a short history is only as tall as its rows"
    );
}

#[test]
fn a_crowded_inbox_still_leaves_history_visible_room() {
    let crowded = history_height(800.0, 1000.0, 900.0, 1.0);
    assert!(crowded > 0.0 && crowded < 800.0, "got {crowded}");
    assert_eq!(
        history_height(30.0, 1000.0, 900.0, 1.0),
        0.0,
        "no room, no history"
    );
}

#[test]
fn the_open_fraction_scales_the_height_and_clamps() {
    let open = history_height(800.0, 250.0, 900.0, 1.0);
    assert_eq!(history_height(800.0, 250.0, 900.0, 0.5), open / 2.0);
    assert_eq!(history_height(800.0, 250.0, 900.0, 0.0), 0.0);
    assert_eq!(history_height(800.0, 250.0, 900.0, 2.0), open);
    assert_eq!(history_height(800.0, 250.0, 900.0, -1.0), 0.0);
}

struct Forge {
    _directory: tempfile::TempDir,
    _listener: std::net::TcpListener,
    core: editor_aqua::core::Core,
    host: editor_aqua::core::HostId,
}

fn silent_forge() -> Forge {
    use artisan_api::tls::{Identity, fingerprint};
    use editor_aqua::core::{Core, EditorFiles, ForgeClient, HostConfig, HostId, HostKind, Screen};
    let directory = tempfile::tempdir().expect("temporary directory");
    let mut core = Core::for_tests(EditorFiles {
        dir: directory.path().join("editor"),
    })
    .expect("core opens");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a Forge that never answers");
    let identity = Identity::generate("inbox-test").expect("identity");
    let pin = fingerprint(identity.certificate_der().expect("certificate").as_ref());
    let config = HostConfig {
        id: HostId("inbox-forge".into()),
        name: "Inbox".into(),
        address: listener.local_addr().expect("address").to_string(),
        fingerprint: pin.clone(),
        kind: HostKind::Local,
    };
    let client = ForgeClient::new(&config.address, &pin, &identity).expect("client");
    core.add_test_host(&config, client);
    core.host_mut(&config.id)
        .expect("host")
        .test_stop_connection();
    core.navigate(Screen::NewThread {
        host: config.id.clone(),
        project: None,
    });
    Forge {
        _directory: directory,
        _listener: listener,
        core,
        host: config.id,
    }
}

impl Forge {
    fn go_live(&mut self) {
        let host = self.host.clone();
        let sender = self
            .core
            .host(&host)
            .expect("host")
            .test_transition_sender();
        editor_aqua::core::runtime::runtime()
            .block_on(sender.status(editor_aqua::core::ConnectionStatus::Live));
        test_support::wait::wait_until(
            "the host goes live",
            std::time::Duration::from_secs(10),
            || {
                self.core.drain_with(|_, _| {});
                self.core.host(&host).expect("host").is_live()
            },
        );
    }

    fn load_empty_page(&mut self) {
        use editor_aqua::core::host::ThreadPageDirection;
        let host = self.host.clone();
        let data = self.core.host_mut(&host).expect("host");
        data.load_threads(false, ThreadPageDirection::Latest);
        let generation = data.test_thread_page_generation(false);
        let sender = data.test_transition_sender();
        editor_aqua::core::runtime::runtime().block_on(sender.thread_page(
            false,
            ThreadPageDirection::Latest,
            0,
            generation,
            artisan_api::ThreadsPage {
                threads: Vec::new(),
                runs: Vec::new(),
                next: None,
                newer: false,
            },
        ));
        test_support::wait::wait_until(
            "the empty page lands",
            std::time::Duration::from_secs(10),
            || {
                self.core.drain_with(|_, _| {});
                self.core
                    .host(&host)
                    .expect("host")
                    .thread_page(false)
                    .is_loaded()
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
        editor_aqua::ui::theme::ArtisanTheme::resolved(),
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
    use editor_aqua::core::client::CallError;
    use editor_aqua::sidebar::{
        controller::{Sidebar, TextSearchStatus},
        view::{ViewAction, dispatch},
    };
    let mut forge = silent_forge();
    let mut sidebar = Sidebar::new(false);
    sidebar.sync(&forge.core, 0);
    sidebar.search.borrow_mut().set_text("needle");
    sidebar.search_text(&forge.core);
    assert_eq!(sidebar.text_search_status(), TextSearchStatus::Searching);
    let key = sidebar.text_search_key().expect("a search runs");
    futures::executor::block_on(sidebar.test_search_failure(
        key,
        "needle".into(),
        CallError::test_inconsistent("/search", "hits point at known items"),
    ));
    sidebar.drain(&mut forge.core, 0);
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
    let mut forge = silent_forge();
    forge.go_live();
    let mut sidebar = Sidebar::new(false);
    sidebar.sync(&forge.core, 0);
    let (_app, texts, _) = rendered(&sidebar, &forge.core);
    assert_eq!(texts, ["Loading threads…"]);

    forge.load_empty_page();
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
