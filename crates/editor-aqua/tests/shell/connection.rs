use editor_aqua::core::{ConnectionStatus, HostConfig, HostId, HostKind, Screen, SettingsPage};
use editor_aqua::shell::application::SectionViews as _;
use editor_aqua::shell::connection::ConnectionPresenter;
use editor_aqua::shell::root::RootViews;
use editor_aqua::ui::theme::ArtisanTheme;
use std::time::{Duration, Instant};
use test_support::editor::core::{connect, test_core};
use test_support::editor::forge::{StubForge, TlsServer, empty_state};
use test_support::editor::shell::{Motion, ThreadShell};

fn forge_whose_stream_always_closes() -> TlsServer {
    use axum::http::header::CONTENT_TYPE;
    use axum::routing::get;
    let router = axum::Router::new()
        .route("/state", get(|| async { axum::Json(empty_state()) }))
        .route(
            "/events",
            get(|| async { ([(CONTENT_TYPE, "text/event-stream")], "") }),
        );
    editor_aqua::core::runtime::runtime().block_on(TlsServer::serve(router))
}

#[test]
fn an_offline_forge_shows_why_and_offers_retry_and_the_forges_page() {
    let directory = tempfile::tempdir().unwrap();
    let mut core = test_core(directory.path());
    let forge = forge_whose_stream_always_closes();
    let config = HostConfig {
        id: HostId("offline-forge".into()),
        name: "Forge".into(),
        address: forge.address().to_owned(),
        fingerprint: forge.fingerprint().to_owned(),
        kind: HostKind::Remote,
    };
    connect(&mut core, &config);
    test_support::wait::wait_until("the host goes offline", Duration::from_secs(10), || {
        core.drain_with(|_, _| {});
        matches!(
            core.host(&config.id).unwrap().status,
            ConnectionStatus::Offline { .. }
        )
    });
    let theme = ArtisanTheme::resolved();
    let mut presenter = ConnectionPresenter::default();
    let start = Instant::now();
    let host = core.host(&config.id).unwrap();
    assert!(presenter.build(host, &theme, start, true).is_some());
    let status = presenter
        .build(host, &theme, start + Duration::from_secs(6), true)
        .expect("an offline Forge has a status");
    let mut app = test_support::editor::ui::mounted(900.0, 700.0, status);
    let texts = test_support::editor::ui::painted_texts(&app);
    for expected in [
        "Last attempt: Forge's event stream closed",
        "Try again now",
        "Forges",
    ] {
        assert!(
            texts.iter().any(|text| text == expected),
            "{expected:?} in {texts:?}"
        );
    }
    test_support::editor::ui::click(&mut app, "manage-forges");
    presenter.dispatch(&mut core);
    assert_eq!(core.screen(), &Screen::Settings(SettingsPage::Hosts));
}

const LOST: &str = "Lost the connection to Forge. Reconnecting…";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shown {
    First,
    Again,
}

fn announces_a_lost_connection(opened: &mut ThreadShell, shown: Shown) -> bool {
    let window = opened.window("connection");
    let element = RootViews::default()
        .build(&mut opened.shell, &mut opened.resources.ui, &window)
        .expect("build the shell");
    let ui = &mut opened.resources.ui;
    match shown {
        Shown::First => ui.mount(|_| element).expect("mount the shell"),
        Shown::Again => ui.refresh(vec![element]).expect("show the shell again"),
    }
    ui.build_frame().expect("lay out the shell");
    ui.tree()
        .emit_semantics()
        .iter()
        .any(|node| node.label == LOST && node.live == Some(aqua_core::LiveRegion::Polite))
}

fn forge_that_never_answers() -> TlsServer {
    use axum::routing::get;
    let router = axum::Router::new().route(
        "/state",
        get(futures::future::pending::<axum::Json<artisan_api::State>>),
    );
    editor_aqua::core::runtime::runtime().block_on(TlsServer::serve(router))
}

#[test]
fn screen_readers_hear_nothing_while_the_editor_first_connects() {
    let forge = forge_that_never_answers();
    let config = HostConfig {
        id: HostId("first-connect".into()),
        name: "Forge".into(),
        address: forge.address().to_owned(),
        fingerprint: forge.fingerprint().to_owned(),
        kind: HostKind::Remote,
    };
    let mut opened = ThreadShell::open_on(config, "first-thread", Motion::Reduced);
    let host = opened.host.id.clone();
    assert_eq!(
        opened.shell.core.host(&host).map(|host| &host.status),
        Some(&ConnectionStatus::Connecting)
    );
    assert!(
        !announces_a_lost_connection(&mut opened, Shown::First),
        "opening the Editor is not announced as a lost connection"
    );
}

#[test]
fn screen_readers_hear_a_lost_connection_once_one_was_lost() {
    let forge = editor_aqua::core::runtime::runtime()
        .block_on(StubForge::serve(empty_state(), axum::Router::new()));
    let mut opened =
        ThreadShell::open_stubbed(&forge, "dropping", "dropped-thread", Motion::Reduced);
    assert!(
        !announces_a_lost_connection(&mut opened, Shown::First),
        "a live Forge is not announced as lost"
    );
    forge.stop_streaming();
    let host = opened.host.id.clone();
    test_support::wait::wait_until(
        "the Editor to notice the dropped stream",
        test_support::wait::LIVENESS,
        || {
            opened.shell.core.drain_with(|_, _| {});
            opened
                .shell
                .core
                .host(&host)
                .is_some_and(|host| host.status != ConnectionStatus::Live)
        },
    );
    assert!(announces_a_lost_connection(&mut opened, Shown::Again));
}
