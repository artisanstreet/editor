use editor_core::{Core, EditorFiles, SetSettingError};
use serde_json::json;

fn files(directory: &tempfile::TempDir) -> EditorFiles {
    EditorFiles {
        dir: directory.path().to_owned(),
    }
}

#[test]
fn unreadable_settings_are_kept_and_block_saves_until_recovered() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("settings.json");
    let original = b"{\"dark_mode\": false, \"font_set\": \"serif\",".to_vec();
    std::fs::write(&path, &original).expect("seed settings.json");
    let mut core = Core::for_tests(files(&directory)).expect("core opens");
    assert!(core.settings.unreadable().is_some());
    assert!(matches!(
        core.set_setting("dark_mode", json!(true)),
        Err(SetSettingError::Unreadable { .. })
    ));
    assert_eq!(std::fs::read(&path).expect("settings.json kept"), original);

    std::fs::write(&path, b"{\"dark_mode\": false, \"font_set\": \"serif\"}")
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
    assert_eq!(saved, json!({"dark_mode": true, "font_set": "serif"}));
}

#[test]
fn a_settings_file_that_breaks_later_is_not_overwritten() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("settings.json");
    let mut core = Core::for_tests(files(&directory)).expect("core opens");
    core.set_setting("dark_mode", json!(true))
        .expect("first save");
    std::fs::write(&path, b"not json").expect("another program breaks the file");
    assert!(core.set_setting("font_set", json!("serif")).is_err());
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
        .set_setting("font_set", json!("serif"))
        .expect("second save");
    let saved: serde_json::Value = serde_json::from_slice(
        &std::fs::read(directory.path().join("settings.json")).expect("settings.json"),
    )
    .expect("json");
    assert_eq!(saved, json!({"dark_mode": false, "font_set": "serif"}));
}
