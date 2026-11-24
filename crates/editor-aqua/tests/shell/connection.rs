use editor_aqua::core::{
    ConnectionStatus, HostConfig, HostId, HostKind, Screen, SettingsPage, WslStatus,
};
use editor_aqua::shell::application::SectionViews as _;
use editor_aqua::shell::connection::ConnectionPresenter;
use editor_aqua::shell::root::RootViews;
use editor_aqua::ui::theme::ArtisanTheme;
use std::time::Duration;
use test_support::editor::core::{connect, test_core};
use test_support::editor::forge::{StubForge, TlsServer, empty_state};
use test_support::editor::shell::{Motion, ThreadShell};

fn forge_whose_stream_always_closes() -> TlsServer {
    use axum::routing::get;
    let router = axum::Router::new()
        .route("/state", get(|| async { axum::Json(empty_state()) }))
        .route(
            "/events",
            get(|| async {
                test_support::http::event_stream([], test_support::http::StreamEnd::Close)
            }),
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
    let manual = test_support::clock::ManualClock::starting_at(1_700_000_000_000);
    let clock = manual.clock();
    let host = core.host(&config.id).unwrap();
    assert!(presenter.build(host, &theme, &clock, true).is_some());
    manual.advance(Duration::from_secs(6));
    let status = presenter
        .build(host, &theme, &clock, true)
        .expect("an offline Forge has a status");
    let mut app = test_support::editor::ui::mounted(900.0, 700.0, status);
    let texts = test_support::editor::ui::painted_texts(&app);
    for expected in [
        "Last attempt: Forge closed its event stream.",
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

fn forge_that_drops_every_connection() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("https://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for connection in listener.incoming() {
            drop(connection);
        }
    });
    address
}

fn wsl_status_after_waiting(
    address: String,
    fingerprint: String,
    settled: fn(&ConnectionStatus) -> bool,
) -> Vec<String> {
    let directory = tempfile::tempdir().unwrap();
    let mut core = test_core(directory.path());
    let config = HostConfig {
        id: HostId("wsl-forge".into()),
        name: "Forge".into(),
        address,
        fingerprint,
        kind: HostKind::Remote,
    };
    connect(&mut core, &config);
    test_support::wait::wait_until("the connection to settle", Duration::from_secs(20), || {
        core.drain_with(|_, _| {});
        settled(&core.host(&config.id).unwrap().status)
    });
    core.host_mut(&config.id).unwrap().wsl = Some(WslStatus {
        distro: "Ubuntu".into(),
        running: true,
        starting: false,
        error: None,
    });
    let theme = ArtisanTheme::resolved();
    let mut presenter = ConnectionPresenter::default();
    let manual = test_support::clock::ManualClock::starting_at(1_700_000_000_000);
    let clock = manual.clock();
    let host = core.host(&config.id).unwrap();
    assert!(presenter.build(host, &theme, &clock, true).is_some());
    manual.advance(Duration::from_secs(6));
    let status = presenter
        .build(host, &theme, &clock, true)
        .expect("a Forge that isn't live has a status");
    test_support::editor::ui::painted_texts(&test_support::editor::ui::mounted(
        900.0, 700.0, status,
    ))
}

#[test]
fn a_wsl_forge_that_is_slow_to_answer_is_not_called_stopped() {
    let forge = forge_that_never_answers();
    let texts = wsl_status_after_waiting(
        forge.address().to_owned(),
        forge.fingerprint().to_owned(),
        |status| *status == ConnectionStatus::Connecting,
    );
    assert!(
        texts
            .iter()
            .any(|text| text.ends_with("Connecting to Artisan Forge…")),
        "{texts:?}"
    );
    assert!(
        texts
            .iter()
            .all(|text| !text.contains("isn't running") && text != "Start Forge"),
        "{texts:?}"
    );
}

#[test]
fn a_wsl_forge_that_cannot_be_reached_offers_to_start_it() {
    let texts = wsl_status_after_waiting(
        forge_that_drops_every_connection(),
        "00".repeat(32),
        |status| matches!(status, ConnectionStatus::Offline { .. }),
    );
    for expected in ["Forge isn't running in Ubuntu.", "Start Forge"] {
        assert!(
            texts.iter().any(|text| text.contains(expected)),
            "{expected:?} in {texts:?}"
        );
    }
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
