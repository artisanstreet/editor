use editor_aqua::core::{ConnectionStatus, Screen, SettingsPage};
use editor_aqua::shell::connection::ConnectionPresenter;
use editor_aqua::ui::theme::ArtisanTheme;
use std::time::{Duration, Instant};
use test_support::editor::core::{connect, remote_host, test_core};

#[test]
fn an_offline_forge_shows_why_and_offers_retry_and_the_forges_page() {
    let directory = tempfile::tempdir().unwrap();
    let mut core = test_core(directory.path());
    let config = remote_host("offline-forge");
    connect(&mut core, &config);
    core.host_mut(&config.id).unwrap().test_stop_connection();
    let sender = core.host(&config.id).unwrap().test_transition_sender();
    editor_aqua::core::runtime::runtime().block_on(sender.status(ConnectionStatus::Offline {
        attempt: 1,
        reason: "dns error: failed to lookup address".into(),
        code: None,
    }));
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
    assert!(presenter.build(host, theme, start, true).is_some());
    let status = presenter
        .build(host, theme, start + Duration::from_secs(6), true)
        .expect("an offline Forge has a status");
    let mut app = test_support::editor::ui::mounted(900.0, 700.0, status);
    let texts = test_support::editor::ui::painted_texts(&app);
    for expected in [
        "Last attempt: dns error: failed to lookup address",
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

#[test]
fn screen_readers_hear_a_lost_connection_only_after_one_was_lost() {
    use editor_aqua::shell::connection::lost;
    assert!(
        lost(&ConnectionStatus::Connecting).is_none(),
        "opening the Editor is not announced as a lost connection"
    );
    assert!(lost(&ConnectionStatus::Live).is_none());
    for interrupted in [
        ConnectionStatus::Offline {
            attempt: 1,
            reason: "Forge's event stream closed".into(),
            code: None,
        },
        ConnectionStatus::Reconnecting { attempt: 1 },
    ] {
        assert!(
            lost(&interrupted).is_some(),
            "{interrupted:?} is announced as a lost connection"
        );
    }
}
