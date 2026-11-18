use artisan_api::{ProjectId, ThreadId};
use editor_core::{HostId, Screen, SettingsPage, SettingsSection};

#[test]
fn links_open_threads_new_threads_and_settings_pages() {
    assert_eq!(
        Screen::from_link("artisan://thread/laptop/thread-1/"),
        Some(Screen::Thread {
            host: HostId("laptop".into()),
            thread: ThreadId("thread-1".into()),
        })
    );
    assert_eq!(
        Screen::from_link("artisan://new/laptop"),
        Some(Screen::NewThread {
            host: HostId("laptop".into()),
            project: None,
        })
    );
    assert_eq!(
        Screen::from_link("artisan://new/laptop/project-1"),
        Some(Screen::NewThread {
            host: HostId("laptop".into()),
            project: Some(ProjectId("project-1".into())),
        })
    );
    assert_eq!(
        Screen::from_link("artisan://settings"),
        Some(Screen::Settings(SettingsPage::General))
    );
    assert_eq!(
        Screen::from_link("artisan://settings/general/appearance"),
        Some(Screen::Settings(SettingsPage::Section(
            SettingsSection::Appearance
        )))
    );
    assert_eq!(
        Screen::from_link("artisan://marketplace"),
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
        assert_eq!(Screen::from_link(link), None, "{link}");
    }
}
