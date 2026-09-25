use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A fresh install root under the temporary directory, removed on drop.
struct Root(PathBuf);

impl Root {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "artisan-editor-settings-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        Self(root)
    }

    fn settings(&self) -> PathBuf {
        settings_path(&self.0)
    }

    fn legacy(&self, name: &str) -> PathBuf {
        self.0.join("ui").join(name)
    }

    fn stored(&self) -> Value {
        serde_json::from_slice(&std::fs::read(self.settings()).unwrap()).unwrap()
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn limit(label: &str) -> FrameRateLimit {
    FrameRateLimit::parse(label).unwrap()
}

#[test]
fn missing_file_loads_defaults_without_writing() {
    let root = Root::new();
    let loaded = load(&root.0);
    assert_eq!(loaded.settings, EditorSettings::default());
    assert_eq!(loaded.path.as_deref(), Some(root.settings().as_path()));
    assert!(loaded.diagnostics.is_empty());
    assert!(
        !root.settings().exists(),
        "loading alone must not create the file"
    );
    let defaults = EditorSettings::default();
    assert_eq!(defaults.frame_rate_limit(), FrameRateLimit::default());
    assert!(defaults.fps_overlay());
    assert_eq!(defaults.reopen_host(), None);
}

#[test]
fn settings_are_under_the_editor_directory_of_the_install_root() {
    let root = Path::new("install-root");
    assert_eq!(
        settings_path(root),
        root.join("editor").join("settings.json")
    );
}

#[test]
fn with_methods_produce_new_values_and_leave_the_original_unchanged() {
    let original = EditorSettings::default();
    let changed = original
        .clone()
        .with_frame_rate_limit(limit("144"))
        .with_fps_overlay(false)
        .with_reopen_host(Some(PathBuf::from("/hosts/ubuntu")));
    assert_eq!(original, EditorSettings::default());
    assert_eq!(changed.frame_rate_limit(), limit("144"));
    assert!(!changed.fps_overlay());
    assert_eq!(changed.reopen_host(), Some(Path::new("/hosts/ubuntu")));
    assert_eq!(
        changed.with_reopen_host(Some(PathBuf::new())).reopen_host(),
        None
    );
}

#[test]
fn persisted_values_round_trip_and_leave_no_temporary_file() {
    let root = Root::new();
    let settings = EditorSettings::default()
        .with_frame_rate_limit(limit("240"))
        .with_fps_overlay(false)
        .with_reopen_host(Some(PathBuf::from("/hosts/ubuntu")));
    storage::persist(&root.settings(), &settings).unwrap();
    assert_eq!(
        root.stored(),
        serde_json::json!({
            "version": SCHEMA_VERSION,
            "frame_rate_limit": "240",
            "fps_overlay": false,
            "reopen_host": "/hosts/ubuntu",
        })
    );
    let entries = std::fs::read_dir(root.settings().parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    assert_eq!(entries, ["settings.json"]);
    assert_eq!(load(&root.0).settings, settings);
    let replaced = settings.with_frame_rate_limit(limit("60"));
    storage::persist(&root.settings(), &replaced).unwrap();
    assert_eq!(load(&root.0).settings, replaced);
}

#[test]
fn unknown_keys_and_newer_versions_survive_a_write() {
    let root = Root::new();
    write(
        &root.settings(),
        br#"{"version": 7, "fps_overlay": false, "theme": {"name": "dusk"}, "font_size": 14}"#,
    );
    let loaded = load(&root.0);
    assert!(loaded.diagnostics.is_empty());
    assert!(!loaded.settings.fps_overlay());
    let changed = loaded.settings.with_frame_rate_limit(limit("120"));
    storage::persist(&root.settings(), &changed).unwrap();
    let stored = root.stored();
    assert_eq!(stored["version"], 7, "a newer schema version is kept");
    assert_eq!(stored["theme"], serde_json::json!({"name": "dusk"}));
    assert_eq!(stored["font_size"], 14);
    assert_eq!(stored["fps_overlay"], false);
    assert_eq!(stored["frame_rate_limit"], "120");
}

#[test]
fn unreadable_known_values_fall_back_individually() {
    let root = Root::new();
    write(
        &root.settings(),
        br#"{"frame_rate_limit": 90, "fps_overlay": "no", "reopen_host": "/hosts/a"}"#,
    );
    let loaded = load(&root.0);
    assert!(loaded.diagnostics.is_empty());
    assert_eq!(
        loaded.settings.frame_rate_limit(),
        FrameRateLimit::default()
    );
    assert!(loaded.settings.fps_overlay());
    assert_eq!(loaded.settings.reopen_host(), Some(Path::new("/hosts/a")));
}

#[test]
fn corrupt_file_is_kept_aside_and_defaults_load() {
    let oversized = vec![b' '; 300 * 1024];
    let cases: [&[u8]; 4] = [b"{ not json", b"[1, 2]", b"null", &oversized];
    for corrupt in cases {
        let root = Root::new();
        write(&root.settings(), corrupt);
        let loaded = load(&root.0);
        assert_eq!(loaded.settings, EditorSettings::default());
        assert_eq!(loaded.diagnostics.len(), 1, "{:?}", loaded.diagnostics);
        assert_eq!(loaded.path.as_deref(), Some(root.settings().as_path()));
        assert!(!root.settings().exists());
        let kept = root.settings().with_extension("json.corrupt");
        assert_eq!(std::fs::read(&kept).unwrap(), corrupt);
        // A later save writes a fresh file and keeps the corrupt copy.
        storage::persist(&root.settings(), &loaded.settings).unwrap();
        assert_eq!(std::fs::read(&kept).unwrap(), corrupt);
        assert_eq!(load(&root.0).settings, EditorSettings::default());
    }
}

#[test]
fn legacy_files_are_imported_once_then_removed() {
    let root = Root::new();
    write(&root.legacy("frame-rate-limit"), b"144\n");
    write(&root.legacy("fps-overlay"), b"false");
    write(&root.legacy("last-used-host"), b"/hosts/ubuntu");
    write(&root.legacy("last-used-model"), b"{}");
    let loaded = load(&root.0);
    assert!(loaded.diagnostics.is_empty(), "{:?}", loaded.diagnostics);
    let expected = EditorSettings::default()
        .with_frame_rate_limit(limit("144"))
        .with_fps_overlay(false)
        .with_reopen_host(Some(PathBuf::from("/hosts/ubuntu")));
    assert_eq!(loaded.settings, expected);
    assert_eq!(load(&root.0).settings, expected, "the import was persisted");
    for name in ["frame-rate-limit", "fps-overlay", "last-used-host"] {
        assert!(!root.legacy(name).exists(), "{name} must be removed");
    }
    assert!(
        root.legacy("last-used-model").exists(),
        "Forge-pool preferences are not part of the Editor pool"
    );
    // Idempotent: loading again changes nothing.
    let before = std::fs::read(root.settings()).unwrap();
    assert_eq!(load(&root.0).settings, expected);
    assert_eq!(std::fs::read(root.settings()).unwrap(), before);
}

#[test]
fn legacy_empty_host_file_imports_the_local_machine() {
    let root = Root::new();
    write(&root.legacy("last-used-host"), b"");
    let loaded = load(&root.0);
    assert_eq!(loaded.settings.reopen_host(), None);
    assert_eq!(root.stored()["reopen_host"], Value::Null);
    assert!(!root.legacy("last-used-host").exists());
}

#[test]
fn stored_values_win_over_stale_legacy_files() {
    let root = Root::new();
    let stored = EditorSettings::default()
        .with_frame_rate_limit(limit("60"))
        .with_reopen_host(Some(PathBuf::from("/hosts/current")));
    storage::persist(&root.settings(), &stored).unwrap();
    write(&root.legacy("frame-rate-limit"), b"240");
    write(&root.legacy("last-used-host"), b"/hosts/stale");
    let loaded = load(&root.0);
    assert_eq!(loaded.settings, stored);
    assert!(!root.legacy("frame-rate-limit").exists());
    assert!(!root.legacy("last-used-host").exists());
}

#[test]
fn invalid_legacy_values_are_not_imported() {
    let root = Root::new();
    write(&root.legacy("frame-rate-limit"), b"200");
    let loaded = load(&root.0);
    assert_eq!(loaded.settings, EditorSettings::default());
    assert!(!root.settings().exists());
    assert!(!root.legacy("frame-rate-limit").exists());
}

#[gpui::test]
fn update_applies_then_persists_a_new_value(cx: &mut gpui::TestAppContext) {
    let root = Root::new();
    cx.update(|cx| {
        assert_eq!(get(cx), &EditorSettings::default());
        install(EditorSettings::default(), Some(root.settings()), cx);
        update(cx, |settings| settings.with_fps_overlay(false)).unwrap();
        assert!(!get(cx).fps_overlay());
        update(cx, |settings| {
            settings.with_reopen_host(Some(PathBuf::from("/hosts/b")))
        })
        .unwrap();
        assert!(!get(cx).fps_overlay(), "earlier changes carry forward");
        assert_eq!(get(cx).reopen_host(), Some(Path::new("/hosts/b")));
    });
    assert_eq!(root.stored()["fps_overlay"], false);
    assert_eq!(root.stored()["reopen_host"], "/hosts/b");
}

#[gpui::test]
fn update_without_a_location_applies_for_the_session_only(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        let result = update(cx, |settings| settings.with_frame_rate_limit(limit("30")));
        assert!(matches!(result, Err(SettingsPersistError::Unavailable)));
        assert_eq!(get(cx).frame_rate_limit(), limit("30"));
    });
}

#[gpui::test]
fn failed_writes_still_apply_for_the_session(cx: &mut gpui::TestAppContext) {
    let root = Root::new();
    // A directory where the settings file belongs makes the rename fail.
    std::fs::create_dir_all(root.settings().join("occupied")).unwrap();
    cx.update(|cx| {
        install(EditorSettings::default(), Some(root.settings()), cx);
        let result = update(cx, |settings| settings.with_fps_overlay(false));
        assert!(matches!(result, Err(SettingsPersistError::Io(_))));
        assert!(!get(cx).fps_overlay());
    });
    assert!(!root.settings().with_extension("json.pending").exists());
}

#[test]
fn legacy_forge_preferences_are_found_then_removed_once_retired() {
    let root = Root::new();
    write(
        &root.legacy("last-used-model"),
        br#"{"version":1,"engine_id":"codex","model_id":"codex-luna","native_model_id":"n"}"#,
    );
    write(
        &root.legacy("project-orders/local"),
        br#"{"version":1,"projects":["p2","p1"]}"#,
    );
    // Editor-pool legacy files are not Forge preferences and stay.
    write(&root.legacy("fps-overlay"), b"false");
    let found = legacy_forge::find(&root.0, None, storage::read_legacy_file);
    assert_eq!(
        found
            .selection
            .as_ref()
            .map(|selection| selection.model_id.as_str()),
        Some("codex-luna")
    );
    assert_eq!(found.project_order.len(), 2);
    found.retire();
    assert!(!root.legacy("last-used-model").exists());
    assert!(!root.legacy("project-orders/local").exists());
    assert!(root.legacy("fps-overlay").exists());
    assert!(legacy_forge::find(&root.0, None, storage::read_legacy_file).is_empty());
}
