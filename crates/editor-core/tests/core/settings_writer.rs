#![cfg(unix)]

use editor_core::{Core, SetSettingError, SettingsSave, SettingsShutdown, drain_settings};
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
        test_support::editor::core::test_core(self.directory.path())
    }

    fn settings(&self) -> PathBuf {
        self.directory.path().join("settings.json")
    }

    fn slow_disk(&self) {
        std::fs::remove_file(self.settings()).expect("remove settings.json");
        fifo(&self.settings());
    }

    fn save_opened(&self) -> File {
        let opened = last_save_opened(&self.settings());
        let next = self.directory.path().join("next-settings.json");
        fifo(&next);
        std::fs::rename(&next, self.settings()).expect("give the next save its own settings.json");
        opened
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

fn fifo(path: &Path) {
    assert!(
        std::process::Command::new("mkfifo")
            .arg(path)
            .status()
            .expect("mkfifo")
            .success(),
        "mkfifo makes settings.json wait for the test"
    );
}

fn last_save_opened(path: &Path) -> File {
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
        .recv_timeout(test_support::wait::LIVENESS)
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
    let first_save = folder.save_opened();
    core.set_setting("browser", json!("third"))
        .expect("the newer change is taken");
    folder.read_only();
    answer(first_save, &json!({"browser": "first"}));
    let second_save = last_save_opened(&folder.settings());
    core.drain();
    core.drain();

    assert_eq!(
        core.settings().get("browser"),
        Some(&json!("third")),
        "the failed older save leaves the newer choice on screen"
    );
    assert!(matches!(core.settings_save(), SettingsSave::Saving));

    folder.writable();
    answer(second_save, &json!({"browser": "first"}));
    drain_until(&mut core, "the newer save to land", |core| {
        matches!(core.settings_save(), SettingsSave::Saved)
    });
    assert_eq!(core.settings().get("browser"), Some(&json!("third")));
    assert_eq!(folder.stored(), json!({"browser": "third"}));
}

#[test]
fn a_rolled_back_change_stays_reported_after_a_later_save_lands() {
    let folder = Folder::new(&json!({"browser": "first", "dark_mode": false}));
    let mut core = folder.core();
    folder.slow_disk();
    core.set_setting("browser", json!("second"))
        .expect("the first change is taken");
    let first_save = folder.save_opened();
    core.set_setting("dark_mode", json!(true))
        .expect("the other change is taken");
    folder.read_only();
    answer(first_save, &json!({"browser": "first", "dark_mode": false}));
    let second_save = last_save_opened(&folder.settings());
    folder.writable();
    answer(
        second_save,
        &json!({"browser": "first", "dark_mode": false, "font_set": "system"}),
    );
    drain_until(&mut core, "the later save to land", |core| {
        core.settings().get("font_set") == Some(&json!("system"))
    });

    assert_eq!(core.settings().get("browser"), Some(&json!("first")));
    assert_eq!(core.settings().get("dark_mode"), Some(&json!(true)));
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
    assert_eq!(core.settings().get("dark_mode"), Some(&json!(false)));
}

fn child_process(test: &str) -> bool {
    if std::env::var("ARTISAN_SETTINGS_SHUTDOWN_TEST").as_deref() == Ok(test) {
        return true;
    }
    let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args(["--exact", test, "--test-threads=1", "--nocapture"])
        .env("ARTISAN_SETTINGS_SHUTDOWN_TEST", test)
        .output()
        .expect("run the shutdown test alone");
    assert!(
        output.status.success(),
        "child settings shutdown failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    false
}

#[test]
fn closing_the_editor_waits_for_every_queued_settings_change() {
    if !child_process("settings_writer::closing_the_editor_waits_for_every_queued_settings_change")
    {
        return;
    }
    let folder = Folder::new(&json!({}));
    let mut core = folder.core();
    folder.slow_disk();
    core.set_setting("dark_mode", json!(true))
        .expect("the first change is taken");
    let first_save = last_save_opened(&folder.settings());
    core.set_setting("browser", json!("helium"))
        .expect("the queued change is taken");
    drop(core);

    editor_core::runtime::runtime().block_on(async {
        let mut shutdown = Box::pin(drain_settings(Duration::from_secs(10)));
        assert!(
            test_support::poll::poll_once(shutdown.as_mut()).is_pending(),
            "shutdown waits while a save is still writing"
        );
        answer(first_save, &json!({}));
        assert_eq!(shutdown.await, SettingsShutdown::Saved);
    });
    assert_eq!(
        folder.stored(),
        json!({"dark_mode": true, "browser": "helium"})
    );
}
