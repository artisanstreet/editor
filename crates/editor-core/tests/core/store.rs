use artisan_api::ThreadId;
use editor_core::{
    Core, ForgeClient, HostConfig, HostId, HostKind, Screen, SetSettingError, SettingsPage,
    SettingsSave, ThreadUnavailable,
};
use serde_json::json;
use std::time::{Duration, Instant};

fn settle(core: &mut Core) {
    test_support::wait::wait_until(
        "the settings save to finish",
        Duration::from_secs(10),
        || {
            core.drain();
            !matches!(core.settings_save(), SettingsSave::Saving)
        },
    );
}

#[test]
fn unreadable_settings_are_kept_and_block_saves_until_recovered() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("settings.json");
    let original = b"{\"dark_mode\": false, \"font_set\": \"system\",".to_vec();
    std::fs::write(&path, &original).expect("seed settings.json");
    let mut core = test_support::editor::core::test_core(directory.path());
    assert!(core.settings().unreadable().is_some());
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
    assert!(core.settings().unreadable().is_none());
    core.set_setting("dark_mode", json!(true))
        .expect("saves resume after recovery");
    settle(&mut core);
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).expect("settings.json")).expect("json");
    assert_eq!(saved, json!({"dark_mode": true, "font_set": "system"}));
}

#[test]
fn a_settings_file_that_breaks_later_is_not_overwritten() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("settings.json");
    let mut core = test_support::editor::core::test_core(directory.path());
    core.set_setting("dark_mode", json!(true))
        .expect("first save");
    settle(&mut core);
    std::fs::write(&path, b"not json").expect("another program breaks the file");
    core.set_setting("font_set", json!("system"))
        .expect("the value is taken while the file is read in the background");
    settle(&mut core);
    assert!(matches!(
        core.settings_save(),
        SettingsSave::Failed(failure) if matches!(**failure, SetSettingError::Unreadable { .. })
    ));
    assert_eq!(
        std::fs::read(&path).expect("settings.json kept"),
        b"not json"
    );
    assert!(core.settings().unreadable().is_some());
}

#[cfg(unix)]
#[derive(Debug, PartialEq, Eq)]
enum SettingsRelease {
    WhileWaiting,
    AfterGivingUp,
}

#[cfg(unix)]
#[test]
fn changing_a_setting_does_not_wait_for_a_slow_disk() {
    use std::io::Write as _;
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("settings.json");
    let mut core = test_support::editor::core::test_core(directory.path());
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .expect("mkfifo")
            .success()
    );
    let (opened_tx, opened) = std::sync::mpsc::channel::<()>();
    let (release_tx, release) = std::sync::mpsc::channel::<()>();
    let slow_disk = std::thread::spawn({
        let path = path.clone();
        move || {
            let mut fifo = std::fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .expect("open the slow settings.json");
            opened_tx.send(()).expect("the test waits for the open");
            let released = match release.recv_timeout(Duration::from_secs(10)) {
                Ok(()) => SettingsRelease::WhileWaiting,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => SettingsRelease::AfterGivingUp,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    unreachable!("the test holds the release sender until it releases")
                }
            };
            fifo.write_all(b"{\"font_set\": \"system\"}")
                .expect("write the slow settings.json");
            released
        }
    });

    core.set_setting("dark_mode", json!(true))
        .expect("the change is taken at once");
    assert_eq!(core.settings().get("dark_mode"), Some(&json!(true)));
    assert!(matches!(core.settings_save(), SettingsSave::Saving));
    opened
        .recv_timeout(Duration::from_secs(10))
        .expect("the save reads settings.json in the background");
    release_tx.send(()).expect("the slow disk is still waiting");
    settle(&mut core);

    assert_eq!(
        slow_disk.join().expect("the slow disk finishes"),
        SettingsRelease::WhileWaiting
    );
    assert!(matches!(core.settings_save(), SettingsSave::Saved));
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).expect("settings.json")).expect("json");
    assert_eq!(saved, json!({"dark_mode": true, "font_set": "system"}));
    assert_eq!(core.settings().get("font_set"), Some(&json!("system")));
}

#[cfg(unix)]
#[test]
fn a_setting_that_cannot_be_saved_says_why_and_goes_back() {
    use std::os::unix::fs::PermissionsExt as _;
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("settings.json");
    std::fs::write(&path, b"{\"dark_mode\": false}").expect("seed settings.json");
    let mut core = test_support::editor::core::test_core(directory.path());
    let writable = std::fs::metadata(directory.path())
        .expect("folder metadata")
        .permissions();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o555))
        .expect("make the settings folder read-only");

    core.set_setting("dark_mode", json!(true))
        .expect("the change is taken at once");
    settle(&mut core);
    std::fs::set_permissions(directory.path(), writable).expect("restore the settings folder");

    assert!(matches!(
        core.settings_save(),
        SettingsSave::Failed(failure)
            if matches!(&**failure, SetSettingError::Save { key, .. } if key == "dark_mode")
    ));
    assert_eq!(core.settings().get("dark_mode"), Some(&json!(false)));
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).expect("settings.json")).expect("json");
    assert_eq!(saved, json!({"dark_mode": false}));
}

#[test]
fn two_editors_changing_different_settings_keep_both() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let mut first = test_support::editor::core::test_core(directory.path());
    let mut second = test_support::editor::core::test_core(directory.path());
    first
        .set_setting("dark_mode", json!(false))
        .expect("first save");
    settle(&mut first);
    second
        .set_setting("font_set", json!("system"))
        .expect("second save");
    settle(&mut second);
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
    let identity = core.identity().cloned().expect("test identity");
    let client =
        ForgeClient::new(&config.address, &config.fingerprint, &identity).expect("test client");
    test_support::editor::core::connect_client(core, &config, client);
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
    let mut core = test_support::editor::core::test_core(directory.path());
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
    let mut core = test_support::editor::core::test_core(directory.path());
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
    let mut core = test_support::editor::core::test_core(directory.path());

    assert_eq!(
        core.settings().get("dark_mode"),
        None,
        "a string where the theme takes a boolean is not the stored theme"
    );
    assert!(core.settings().rejected("dark_mode").is_some());
    assert_eq!(core.settings().get("streaming_mode"), None);
    assert!(core.settings().rejected("streaming_mode").is_some());
    assert_eq!(core.settings().get("graphics_vsync"), Some(&json!(false)));
    assert_eq!(core.settings().get("font_set"), Some(&json!("system")));

    assert!(matches!(
        core.set_setting("streaming_mode", json!("fast")),
        Err(SetSettingError::Invalid { .. })
    ));
    core.set_setting("dark_mode", json!(false))
        .expect("a valid value replaces the broken one");
    assert_eq!(core.settings().get("dark_mode"), Some(&json!(false)));
    assert!(core.settings().rejected("dark_mode").is_none());
}

#[test]
fn opening_a_thread_its_forge_cannot_send_says_why() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let mut core = test_support::editor::core::test_core(directory.path());
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
    let ThreadUnavailable::Failed(error) = &unavailable else {
        panic!("the lookup failed rather than finding the thread deleted: {unavailable}");
    };
    assert!(
        error.to_string().contains("unknown-thread"),
        "the logged error names the thread: {error}"
    );
    let shown = unavailable.to_string();
    assert!(
        shown.starts_with("Couldn't load this thread. ") && !shown.contains("unknown-thread"),
        "the message says why in words, without the error chain: {shown}"
    );
}

#[cfg(unix)]
#[test]
fn settings_edit_the_forge_on_screen_when_the_last_forge_cannot_be_saved() {
    use std::os::unix::fs::PermissionsExt as _;
    let directory = tempfile::tempdir().expect("temporary directory");
    let mut core = test_support::editor::core::test_core(directory.path());
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
