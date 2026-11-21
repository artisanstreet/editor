use artisan_api::{EngineId, ProjectId, ThreadId};
use editor_core::{DeepLinkError, HostId, Screen, SettingsPage, SettingsSection};

#[test]
fn deep_links_open_the_screens_they_name() {
    assert_eq!(
        Screen::from_link("artisan://thread/mch_01/0195f1a2-thread").expect("thread link"),
        Screen::Thread {
            host: HostId("mch_01".into()),
            thread: "0195f1a2-thread".into(),
        }
    );
    assert_eq!(
        Screen::from_link("artisan://new/127-0-0-1-8080-abcdef12/").expect("new-thread link"),
        Screen::NewThread {
            host: HostId("127-0-0-1-8080-abcdef12".into()),
            project: None,
        }
    );
    assert_eq!(
        Screen::from_link("artisan://settings/engines/claude/account").expect("settings link"),
        Screen::Settings(SettingsPage::Section(SettingsSection::Account(
            EngineId::Claude
        )))
    );
    assert_eq!(
        Screen::from_link("artisan://settings").expect("settings link"),
        Screen::Settings(SettingsPage::General)
    );
}

#[test]
fn deep_links_with_missing_or_malformed_ids_open_nothing() {
    for link in [
        "artisan://thread//thread-id",
        "artisan://thread/host-id/",
        "artisan://thread/host-id/thread-id?focus=1",
        "artisan://thread/host-id/thread-id#item",
        "artisan://thread/host%20id/thread-id",
        "artisan://thread/host-id/thread-id/extra",
        "artisan://new/",
        "artisan://settings/nowhere",
        "artisan://elsewhere",
    ] {
        assert!(
            matches!(Screen::from_link(link), Err(DeepLinkError::Refused { .. })),
            "{link} must not open a screen"
        );
    }
    assert!(matches!(
        Screen::from_link("--source-encoded"),
        Err(DeepLinkError::NotALink)
    ));
}

#[test]
fn links_open_threads_new_threads_and_settings_pages() {
    assert_eq!(
        Screen::from_link("artisan://thread/laptop/thread-1/").ok(),
        Some(Screen::Thread {
            host: HostId("laptop".into()),
            thread: ThreadId("thread-1".into()),
        })
    );
    assert_eq!(
        Screen::from_link("artisan://new/laptop").ok(),
        Some(Screen::NewThread {
            host: HostId("laptop".into()),
            project: None,
        })
    );
    assert_eq!(
        Screen::from_link("artisan://new/laptop/project-1").ok(),
        Some(Screen::NewThread {
            host: HostId("laptop".into()),
            project: Some(ProjectId("project-1".into())),
        })
    );
    assert_eq!(
        Screen::from_link("artisan://settings").ok(),
        Some(Screen::Settings(SettingsPage::General))
    );
    assert_eq!(
        Screen::from_link("artisan://settings/general/appearance").ok(),
        Some(Screen::Settings(SettingsPage::Section(
            SettingsSection::Appearance
        )))
    );
    assert_eq!(
        Screen::from_link("artisan://marketplace").ok(),
        Some(Screen::Marketplace)
    );
}

#[test]
fn malformed_links_open_nothing() {
    for link in [
        "https://thread/laptop/thread-1",
        "artisan://",
        "artisan://thread/laptop",
        "artisan://thread//thread-1",
        "artisan://thread/laptop//",
        "artisan://new//project-1",
        "artisan://settings/nowhere",
        "artisan://thread/laptop/thread-1/extra",
    ] {
        assert!(Screen::from_link(link).is_err(), "{link}");
    }
}
