use artisan_api::ThreadId;
use editor_core::{
    Core, EditorFiles, ForgeClient, HostConfig, HostId, HostKind, Screen, SetSettingError,
    SettingsPage, ThreadUnavailable,
};
use serde_json::json;
use std::time::{Duration, Instant};

fn files(directory: &tempfile::TempDir) -> EditorFiles {
    EditorFiles {
        dir: directory.path().to_owned(),
    }
}

#[test]
fn unreadable_settings_are_kept_and_block_saves_until_recovered() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("settings.json");
    let original = b"{\"dark_mode\": false, \"font_set\": \"system\",".to_vec();
    std::fs::write(&path, &original).expect("seed settings.json");
    let mut core = Core::for_tests(files(&directory)).expect("core opens");
    assert!(core.settings.unreadable().is_some());
    assert!(matches!(
        core.set_setting("dark_mode", json!(true)),
        Err(SetSettingError::Unreadable { .. })
    ));
    assert_eq!(std::fs::read(&path).expect("settings.json kept"), original);

    std::fs::write(&path, b"{\"dark_mode\": false, \"font_set\": \"system\"}")
        .expect("user repairs settings.json");
    assert!(matches!(
        core.set_setting("dark_mode", json!(true)),
        Err(SetSettingError::Unreadable { .. })
    ));
    core.reload_settings();
    assert!(core.settings.unreadable().is_none());
    core.set_setting("dark_mode", json!(true))
        .expect("saves resume after recovery");
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).expect("settings.json")).expect("json");
    assert_eq!(saved, json!({"dark_mode": true, "font_set": "system"}));
}

#[test]
fn a_settings_file_that_breaks_later_is_not_overwritten() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("settings.json");
    let mut core = Core::for_tests(files(&directory)).expect("core opens");
    core.set_setting("dark_mode", json!(true))
        .expect("first save");
    std::fs::write(&path, b"not json").expect("another program breaks the file");
    assert!(core.set_setting("font_set", json!("system")).is_err());
    assert_eq!(
        std::fs::read(&path).expect("settings.json kept"),
        b"not json"
    );
    assert!(core.settings.unreadable().is_some());
}

#[test]
fn two_editors_changing_different_settings_keep_both() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let mut first = Core::for_tests(files(&directory)).expect("first core");
    let mut second = Core::for_tests(files(&directory)).expect("second core");
    first
        .set_setting("dark_mode", json!(false))
        .expect("first save");
    second
        .set_setting("font_set", json!("system"))
        .expect("second save");
    let saved: serde_json::Value = serde_json::from_slice(
        &std::fs::read(directory.path().join("settings.json")).expect("settings.json"),
    )
    .expect("json");
    assert_eq!(saved, json!({"dark_mode": false, "font_set": "system"}));
}

fn forge(core: &mut Core, id: &str) -> HostId {
    let config = HostConfig {
        id: HostId(id.to_owned()),
        name: id.to_owned(),
        address: "127.0.0.1:1".to_owned(),
        fingerprint: "0".repeat(64),
        kind: HostKind::Remote,
    };
    let identity = core.identity.clone().expect("test identity");
    let client =
        ForgeClient::new(&config.address, &config.fingerprint, &identity).expect("test client");
    core.add_test_host(&config, client);
    config.id
}

fn new_thread(host: &HostId) -> Screen {
    Screen::NewThread {
        host: host.clone(),
        project: None,
    }
}

#[test]
fn settings_opened_after_going_back_edit_the_forge_on_screen() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let mut core = Core::for_tests(files(&directory)).expect("core opens");
    let first = forge(&mut core, "first-forge");
    let second = forge(&mut core, "second-forge");

    core.navigate(new_thread(&first));
    core.navigate(new_thread(&second));
    core.go_back();
    assert_eq!(core.screen(), &new_thread(&first));
    core.navigate(Screen::Settings(SettingsPage::GlobalPrompt));

    assert_eq!(
        core.active_host().map(|host| host.id.clone()),
        Some(first),
        "Settings must edit the Forge whose workspace the user went back to"
    );
}

#[test]
fn removing_a_forge_drops_it_from_back_navigation() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let mut core = Core::for_tests(files(&directory)).expect("core opens");
    let kept = forge(&mut core, "kept-forge");
    let removed = forge(&mut core, "removed-forge");

    core.navigate(new_thread(&kept));
    core.navigate(Screen::Thread {
        host: removed.clone(),
        thread: ThreadId("removed-thread".into()),
    });
    core.navigate(Screen::Settings(SettingsPage::Hosts));
    core.remove_host(&removed)
        .expect("the pairing is forgotten");

    for _ in 0..4 {
        assert_ne!(
            core.screen().host(),
            Some(&removed),
            "Back reopened a screen of the removed Forge"
        );
        core.go_back();
    }
    assert_eq!(core.screen(), &new_thread(&kept));
}

#[test]
fn stored_settings_that_break_their_declaration_are_ignored_everywhere() {
    let directory = tempfile::tempdir().expect("temporary directory");
    std::fs::write(
        directory.path().join("settings.json"),
        br#"{"dark_mode": "false", "streaming_mode": "fast", "graphics_vsync": false, "font_set": "system"}"#,
    )
    .expect("seed settings.json");
    let mut core = Core::for_tests(files(&directory)).expect("core opens");

    assert_eq!(
        core.settings.get("dark_mode"),
        None,
        "a string where the theme takes a boolean is not the stored theme"
    );
    assert!(core.settings.rejected("dark_mode").is_some());
    assert_eq!(core.settings.get("streaming_mode"), None);
    assert!(core.settings.rejected("streaming_mode").is_some());
    assert_eq!(core.settings.get("graphics_vsync"), Some(&json!(false)));
    assert_eq!(core.settings.get("font_set"), Some(&json!("system")));

    assert!(matches!(
        core.set_setting("streaming_mode", json!("fast")),
        Err(SetSettingError::Invalid { .. })
    ));
    core.set_setting("dark_mode", json!(false))
        .expect("a valid value replaces the broken one");
    assert_eq!(core.settings.get("dark_mode"), Some(&json!(false)));
    assert!(core.settings.rejected("dark_mode").is_none());
}

#[test]
fn opening_a_thread_its_forge_cannot_send_says_why() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let mut core = Core::for_tests(files(&directory)).expect("core opens");
    let host = forge(&mut core, "silent-forge");
    let thread = ThreadId("unknown-thread".into());
    core.navigate(Screen::Thread {
        host: host.clone(),
        thread: thread.clone(),
    });

    let deadline = Instant::now() + Duration::from_secs(20);
    let unavailable = loop {
        core.drain_with(|_, _| {});
        if let Some(unavailable) = core
            .host(&host)
            .and_then(|forge| forge.thread_unavailable(&thread))
        {
            break unavailable.clone();
        }
        assert!(
            Instant::now() < deadline,
            "the failed thread lookup never reached the screen"
        );
        std::thread::yield_now();
    };
    assert!(matches!(unavailable, ThreadUnavailable::Failed(_)));
    assert!(
        unavailable.to_string().contains("unknown-thread"),
        "the message names the thread: {unavailable}"
    );
}

#[cfg(unix)]
#[test]
fn settings_edit_the_forge_on_screen_when_the_last_forge_cannot_be_saved() {
    use std::os::unix::fs::PermissionsExt as _;
    let directory = tempfile::tempdir().expect("temporary directory");
    let mut core = Core::for_tests(files(&directory)).expect("core opens");
    let first = forge(&mut core, "first-forge");
    let _second = forge(&mut core, "second-forge");
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o500))
        .expect("make the data directory read-only");

    core.navigate(new_thread(&first));
    core.navigate(Screen::Settings(SettingsPage::GlobalPrompt));
    let active = core.active_host().map(|host| host.id.clone());

    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
        .expect("restore the data directory");
    assert_eq!(
        active,
        Some(first),
        "a failed save of the last Forge must not move Settings to another Forge"
    );
}
