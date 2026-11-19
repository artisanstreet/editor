#![cfg(unix)]

use editor_core::{Core, EditorFiles, SetSettingError, SettingsSave};
use serde_json::json;
use std::fs::File;
use std::io::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

struct Folder {
    directory: tempfile::TempDir,
    writable: std::fs::Permissions,
}

impl Folder {
    fn new(seed: &serde_json::Value) -> Self {
        let directory = tempfile::tempdir().expect("temporary directory");
        std::fs::write(
            directory.path().join("settings.json"),
            serde_json::to_vec(seed).expect("encode the seed"),
        )
        .expect("seed settings.json");
        let writable = std::fs::metadata(directory.path())
            .expect("folder metadata")
            .permissions();
        Self {
            directory,
            writable,
        }
    }

    fn core(&self) -> Core {
        Core::for_tests(EditorFiles {
            dir: self.directory.path().to_owned(),
        })
        .expect("core opens")
    }

    fn settings(&self) -> PathBuf {
        self.directory.path().join("settings.json")
    }

    fn slow_disk(&self) {
        std::fs::remove_file(self.settings()).expect("remove settings.json");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(self.settings())
                .status()
                .expect("mkfifo")
                .success(),
            "mkfifo makes settings.json wait for the test"
        );
    }

    fn read_only(&self) {
        std::fs::set_permissions(
            self.directory.path(),
            std::fs::Permissions::from_mode(0o555),
        )
        .expect("make the settings folder read-only");
    }

    fn writable(&self) {
        std::fs::set_permissions(self.directory.path(), self.writable.clone())
            .expect("restore the settings folder");
    }

    fn stored(&self) -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(self.settings()).expect("settings.json"))
            .expect("settings.json is json")
    }
}

fn reader_opened(path: &Path) -> File {
    let (opened, writer) = std::sync::mpsc::channel();
    let path = path.to_owned();
    std::thread::spawn(move || {
        let fifo = std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .expect("open the slow settings.json");
        opened.send(fifo).expect("the test waits for the save");
    });
    writer
        .recv_timeout(Duration::from_secs(10))
        .expect("a save reads settings.json")
}

fn answer(mut fifo: File, stored: &serde_json::Value) {
    fifo.write_all(&serde_json::to_vec(stored).expect("encode settings"))
        .expect("answer the save");
}

fn drain_until(core: &mut Core, what: &str, mut done: impl FnMut(&Core) -> bool) {
    test_support::wait::wait_until(what, Duration::from_secs(10), || {
        core.drain();
        done(core)
    });
}

#[test]
fn an_older_failed_save_does_not_undo_a_newer_choice() {
    let folder = Folder::new(&json!({"browser": "first"}));
    let mut core = folder.core();
    folder.slow_disk();
    core.set_setting("browser", json!("second"))
        .expect("the first change is taken");
    let first_save = reader_opened(&folder.settings());
    core.set_setting("browser", json!("third"))
        .expect("the newer change is taken");
    folder.read_only();
    answer(first_save, &json!({"browser": "first"}));
    let second_save = reader_opened(&folder.settings());
    core.drain();
    core.drain();

    assert_eq!(
        core.settings.get("browser"),
        Some(&json!("third")),
        "the failed older save leaves the newer choice on screen"
    );
    assert!(matches!(core.settings_save(), SettingsSave::Saving));

    folder.writable();
    answer(second_save, &json!({"browser": "first"}));
    drain_until(&mut core, "the newer save to land", |core| {
        matches!(core.settings_save(), SettingsSave::Saved)
    });
    assert_eq!(core.settings.get("browser"), Some(&json!("third")));
    assert_eq!(folder.stored(), json!({"browser": "third"}));
}

#[test]
fn a_rolled_back_change_stays_reported_after_a_later_save_lands() {
    let folder = Folder::new(&json!({"browser": "first", "dark_mode": false}));
    let mut core = folder.core();
    folder.slow_disk();
    core.set_setting("browser", json!("second"))
        .expect("the first change is taken");
    let first_save = reader_opened(&folder.settings());
    core.set_setting("dark_mode", json!(true))
        .expect("the other change is taken");
    folder.read_only();
    answer(first_save, &json!({"browser": "first", "dark_mode": false}));
    let second_save = reader_opened(&folder.settings());
    folder.writable();
    answer(
        second_save,
        &json!({"browser": "first", "dark_mode": false, "font_set": "system"}),
    );
    drain_until(&mut core, "the later save to land", |core| {
        core.settings.get("font_set") == Some(&json!("system"))
    });

    assert_eq!(core.settings.get("browser"), Some(&json!("first")));
    assert_eq!(core.settings.get("dark_mode"), Some(&json!(true)));
    assert!(
        matches!(
            core.settings_save(),
            SettingsSave::Failed(failure)
                if matches!(&**failure, SetSettingError::Save { key, .. } if key == "browser")
        ),
        "the indicator keeps saying the browser change was lost, not Saved"
    );
    assert_eq!(
        folder.stored(),
        json!({"browser": "first", "dark_mode": true, "font_set": "system"})
    );
}

#[test]
fn reloading_settings_clears_a_failed_save() {
    let folder = Folder::new(&json!({"dark_mode": false}));
    let mut core = folder.core();
    folder.read_only();
    core.set_setting("dark_mode", json!(true))
        .expect("the change is taken at once");
    drain_until(&mut core, "the save to fail", |core| {
        matches!(core.settings_save(), SettingsSave::Failed(_))
    });
    folder.writable();

    core.reload_settings();

    assert!(
        matches!(core.settings_save(), SettingsSave::Saved),
        "Reload settings does not show the old failure again"
    );
    assert_eq!(core.settings.get("dark_mode"), Some(&json!(false)));
}
