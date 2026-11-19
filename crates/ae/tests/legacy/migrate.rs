#![cfg(unix)]

use super::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command};

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

struct Machine {
    _root: tempfile::TempDir,
    root: PathBuf,
    old: Old,
}

impl Machine {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().to_owned();
        let old = Old::unix(&path.join("data"), &path.join("config")).unwrap();
        Self {
            _root: root,
            root: path,
            old,
        }
    }

    fn refresh(&mut self) {
        self.old = Old::unix(&self.root.join("data"), &self.root.join("config")).unwrap();
    }

    fn editor(&self) -> DataMove {
        DataMove {
            data: Data::Editor,
            sources: self.old.editor_data.clone(),
            destination: self.root.join("street/editor/data"),
        }
    }

    fn forge(&self) -> DataMove {
        DataMove {
            data: Data::Forge,
            sources: self.old.forge_data.clone(),
            destination: self.root.join("street/forge/data"),
        }
    }

    fn run(&self, done: &[App], moves: &[DataMove]) -> Result<Vec<String>, MigrateError> {
        Work {
            old: &self.old,
            done,
            moves,
        }
        .run()
    }
}

#[test]
fn settings_already_in_place_leave_every_earlier_copy_alone() {
    let machine = Machine::new();
    let editor = machine.editor();
    write(&editor.destination.join("settings.json"), "{\"new\":true}");
    write(&editor.sources[0].join("identity.json"), "release identity");
    write(&editor.sources[1].join("hosts.json"), "dev hosts");

    machine.run(&[App::Ae, App::Editor], &[editor]).unwrap();

    let editor = machine.editor();
    assert_eq!(
        read(&editor.destination.join("settings.json")),
        "{\"new\":true}"
    );
    assert_eq!(
        read(&editor.sources[0].join("identity.json")),
        "release identity"
    );
    assert_eq!(read(&editor.sources[1].join("hosts.json")), "dev hosts");
}

#[test]
fn a_destination_with_only_logs_is_kept_aside_and_older_copies_survive() {
    let machine = Machine::new();
    let editor = machine.editor();
    write(&editor.destination.join("logs/editor.log"), "a log");
    write(
        &editor.sources[1].join("hosts.json"),
        "[\"https://127.0.0.1:47901\"]",
    );
    write(&editor.sources[1].join("settings.json"), "dev settings");
    write(&editor.sources[0].join("identity.json"), "older identity");
    let past = SystemTime::now() - std::time::Duration::from_secs(3600);
    fs::File::options()
        .write(true)
        .open(editor.sources[0].join("identity.json"))
        .unwrap()
        .set_modified(past)
        .unwrap();

    let moved = machine.run(&[App::Ae, App::Editor], &[editor]).unwrap();

    let editor = machine.editor();
    assert_eq!(moved.len(), 1);
    assert_eq!(
        read(&editor.destination.join("settings.json")),
        "dev settings"
    );
    assert_eq!(
        read(&editor.destination.join("hosts.json")),
        "[\"https://127.0.0.1:47900\"]"
    );
    assert!(!editor.destination.join(RECORD).exists());
    assert!(!editor.destination.join("logs").exists(), "nothing merged");
    let aside: Vec<PathBuf> = fs::read_dir(editor.destination.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.to_string_lossy().contains(".before-migration-"))
        .collect();
    assert_eq!(aside.len(), 1);
    assert_eq!(read(&aside[0].join("logs/editor.log")), "a log");
    assert!(!editor.sources[1].exists());
    assert_eq!(
        read(&editor.sources[0].join("identity.json")),
        "older identity"
    );
}

#[test]
fn an_interrupted_copy_is_redone_from_the_intact_source() {
    let machine = Machine::new();
    let forge = machine.forge();
    write(&forge.sources[0].join("forge.sqlite3"), "database");
    write(
        &forge.sources[0].join("engines/claude/settings.json"),
        "engine",
    );
    write(&forge.staging().join("forge.sqlite3"), "database");

    assert!(forge.pending().unwrap());
    machine.run(&[App::Ae, App::Forge], &[forge]).unwrap();

    let forge = machine.forge();
    assert_eq!(read(&forge.destination.join("forge.sqlite3")), "database");
    assert_eq!(
        read(&forge.destination.join("engines/claude/settings.json")),
        "engine"
    );
    assert!(!forge.staging().exists());
    assert!(!forge.sources[0].exists());
    assert!(!forge.pending().unwrap());
}

#[test]
fn a_complete_staging_whose_source_is_gone_is_put_in_place() {
    let machine = Machine::new();
    let forge = machine.forge();
    let staging = forge.staging();
    write(&staging.join("forge.sqlite3"), "database");
    write(&staging.join("forge.pid"), "123");
    write_record(&staging, &forge.sources[1]).unwrap();

    machine.run(&[App::Ae, App::Forge], &[forge]).unwrap();

    let forge = machine.forge();
    assert_eq!(read(&forge.destination.join("forge.sqlite3")), "database");
    assert!(!forge.destination.join("forge.pid").exists());
    assert!(!forge.destination.join(RECORD).exists());
    assert!(!staging.exists());
}

#[test]
fn staged_old_data_goes_back_when_forge_already_has_its_own() {
    let machine = Machine::new();
    let forge = machine.forge();
    let staging = forge.staging();
    write(&staging.join("forge.sqlite3"), "earlier database");
    write_record(&staging, &forge.sources[1]).unwrap();
    write(&forge.destination.join("forge.sqlite3"), "new database");

    assert!(forge.pending().unwrap(), "the staged data is not forgotten");
    machine.run(&[App::Ae, App::Forge], &[forge]).unwrap();

    let forge = machine.forge();
    assert_eq!(
        read(&forge.destination.join("forge.sqlite3")),
        "new database"
    );
    assert_eq!(
        read(&forge.sources[1].join("forge.sqlite3")),
        "earlier database",
        "the earlier data is back where it came from"
    );
    assert!(!forge.sources[1].join(RECORD).exists());
    assert!(!staging.exists());
    assert!(!forge.pending().unwrap());
}

#[test]
fn staged_old_data_is_set_aside_when_its_old_place_is_taken_again() {
    let machine = Machine::new();
    let forge = machine.forge();
    let staging = forge.staging();
    write(&staging.join("forge.sqlite3"), "earlier database");
    write_record(&staging, &forge.sources[1]).unwrap();
    write(
        &forge.sources[1].join("forge.sqlite3"),
        "recreated database",
    );
    write(&forge.destination.join("forge.sqlite3"), "new database");

    machine.run(&[App::Ae, App::Forge], &[forge]).unwrap();

    let forge = machine.forge();
    assert_eq!(
        read(&forge.sources[1].join("forge.sqlite3")),
        "recreated database"
    );
    let parent = forge.destination.parent().unwrap();
    let aside: Vec<PathBuf> = fs::read_dir(parent)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.to_string_lossy().contains("before-migration"))
        .collect();
    assert_eq!(aside.len(), 1, "{aside:?}");
    assert_eq!(read(&aside[0].join("forge.sqlite3")), "earlier database");
    assert!(!aside[0].join(RECORD).exists());
    assert!(!staging.exists());
}

fn old_programs(machine: &Machine) -> Vec<PathBuf> {
    let data = machine.root.join("data");
    let paths = [
        data.join("artisan/bin/ae"),
        data.join("artisan/versions/0.3.0/ae"),
        data.join("artisan/versions/0.3.0/artisan-editor"),
        data.join("artisan-dev/bin/artisan-editor"),
        data.join("artisan-dev/editor.log"),
    ];
    for path in &paths {
        write(path, "old");
    }
    paths.to_vec()
}

#[test]
fn installing_forge_alone_leaves_the_earlier_editor_installed() {
    let mut machine = Machine::new();
    let programs = old_programs(&machine);
    machine.refresh();

    machine.run(&[App::Ae, App::Forge], &[]).unwrap();

    assert!(!programs[0].exists(), "the old ae went with Forge and ae");
    for kept in &programs[1..] {
        assert!(
            kept.exists(),
            "{} belongs to the Editor too",
            kept.display()
        );
    }

    machine
        .run(&[App::Ae, App::Forge, App::Editor], &[])
        .unwrap();
    for removed in &programs {
        assert!(!removed.exists(), "{} stayed", removed.display());
    }
}

fn sleep_as(executable: &Path) -> Child {
    let sleep = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|dir| dir.join("sleep"))
        .find(|path| path.is_file())
        .unwrap();
    fs::create_dir_all(executable.parent().unwrap()).unwrap();
    test_support::executable::install(&sleep, executable);
    Command::new(executable)
        .arg0("sleep")
        .arg("600")
        .spawn()
        .unwrap()
}

fn record_pid(pid_file: &Path, child: &Child) {
    write(pid_file, &child.id().to_string());
}

#[test]
fn only_a_forge_that_wrote_its_pid_file_is_stopped() {
    let machine = Machine::new();
    let elsewhere = machine.root.join("elsewhere");
    let mut forge = sleep_as(&elsewhere.join("forge"));
    let mut reused = sleep_as(&elsewhere.join("sleep"));
    let mut stale = sleep_as(&elsewhere.join("ae"));
    record_pid(&machine.old.forge_data[0].join("forge.pid"), &forge);
    record_pid(&machine.old.forge_data[1].join("forge.pid"), &reused);
    let stale_file = machine.root.join("data/artisan/forge/forge.pid.stale");
    record_pid(&stale_file, &stale);
    fs::File::options()
        .write(true)
        .open(&stale_file)
        .unwrap()
        .set_modified(SystemTime::now() - std::time::Duration::from_secs(3600))
        .unwrap();

    assert!(old_forge(&stale_file).unwrap().is_none());
    machine.run(&[App::Ae, App::Forge], &[]).unwrap();

    assert!(
        !forge.wait().unwrap().success(),
        "the old Forge was stopped"
    );
    assert!(
        reused.try_wait().unwrap().is_none(),
        "an unrelated process kept running"
    );
    assert!(
        stale.try_wait().unwrap().is_none(),
        "a process newer than its pid file kept running"
    );
    reused.kill().unwrap();
    stale.kill().unwrap();
    reused.wait().unwrap();
    stale.wait().unwrap();
}

fn codex_state(root: &Path, rows: &[(&str, String)]) -> PathBuf {
    let path = root.join(CODEX_HOME).join("state_5.sqlite");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch(include_str!("codex_threads.sql")).unwrap();
    for (id, rollout) in rows {
        db.execute(
            include_str!("insert_codex_thread.sql"),
            rusqlite::params![id, rollout],
        )
        .unwrap();
    }
    path
}

fn rollout(root: &Path) -> String {
    root.join(CODEX_HOME)
        .join("sessions/2026/10/17/rollout-a.jsonl")
        .display()
        .to_string()
}

fn rollout_of(state: &Path, id: &str) -> String {
    rusqlite::Connection::open(state)
        .unwrap()
        .query_row(include_str!("rollout_path.sql"), [id], |row| row.get(0))
        .unwrap()
}

#[test]
fn codex_threads_find_their_rollouts_after_a_failed_retarget_is_retried() {
    let machine = Machine::new();
    let forge = machine.forge();
    let source = forge.sources[1].clone();
    write(&source.join("forge.sqlite3"), "database");
    let elsewhere = machine
        .root
        .join("other/rollout-b.jsonl")
        .display()
        .to_string();
    let state = codex_state(
        &source,
        &[("a", rollout(&source)), ("b", elsewhere.clone())],
    );
    fs::set_permissions(&state, fs::Permissions::from_mode(0o444)).unwrap();

    let failed = machine.run(&[App::Ae, App::Forge], &[forge]);
    assert!(
        matches!(failed, Err(MigrateError::Retarget { .. })),
        "{failed:?}"
    );

    let forge = machine.forge();
    let moved = forge.destination.join(CODEX_HOME).join("state_5.sqlite");
    fs::set_permissions(&moved, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(forge.pending().unwrap());
    machine.run(&[App::Ae, App::Forge], &[forge]).unwrap();

    let forge = machine.forge();
    assert_eq!(rollout_of(&moved, "a"), rollout(&forge.destination));
    assert_eq!(rollout_of(&moved, "b"), elsewhere);
    assert!(!forge.destination.join(RECORD).exists());
    assert!(!forge.pending().unwrap());
}

#[test]
fn an_unsupported_codex_schema_stops_before_anything_moves() {
    let machine = Machine::new();
    let forge = machine.forge();
    let source = forge.sources[0].clone();
    write(&source.join("forge.sqlite3"), "database");
    let state = source.join(CODEX_HOME).join("state_9.sqlite");
    fs::create_dir_all(state.parent().unwrap()).unwrap();
    rusqlite::Connection::open(&state)
        .unwrap()
        .execute_batch(include_str!("unsupported_codex_threads.sql"))
        .unwrap();

    let failed = machine.run(&[App::Ae, App::Forge], &[forge]);

    assert!(
        matches!(failed, Err(MigrateError::UnsupportedCodexSchema { .. })),
        "{failed:?}"
    );
    let forge = machine.forge();
    assert_eq!(read(&source.join("forge.sqlite3")), "database");
    assert!(!source.join(RECORD).exists());
    assert!(!forge.destination.exists());
    assert!(!forge.staging().exists());
}

#[test]
fn a_link_that_cannot_be_repointed_fails_the_move_and_stays_intact() {
    let machine = Machine::new();
    let forge = machine.forge();
    let source = forge.sources[0].clone();
    write(&source.join("forge.sqlite3"), "database");
    write(&source.join("skills/shared/SKILL.md"), "skill");
    let links = source.join("engines/claude/skills");
    fs::create_dir_all(&links).unwrap();
    std::os::unix::fs::symlink(source.join("skills/shared"), links.join("shared")).unwrap();
    fs::set_permissions(&links, fs::Permissions::from_mode(0o555)).unwrap();

    let failed = machine.run(&[App::Ae, App::Forge], &[forge]);

    assert!(
        matches!(failed, Err(MigrateError::Relink { .. })),
        "{failed:?}"
    );
    let forge = machine.forge();
    let staged = forge.staging().join("engines/claude/skills");
    assert_eq!(
        fs::read_link(staged.join("shared")).unwrap(),
        source.join("skills/shared")
    );
    fs::set_permissions(&staged, fs::Permissions::from_mode(0o755)).unwrap();

    machine.run(&[App::Ae, App::Forge], &[forge]).unwrap();

    let forge = machine.forge();
    let link = forge.destination.join("engines/claude/skills/shared");
    assert_eq!(
        fs::read_link(&link).unwrap(),
        forge.destination.join("skills/shared")
    );
    assert_eq!(read(&link.join("SKILL.md")), "skill");
}

#[test]
fn copying_across_volumes_keeps_links_as_links() {
    let dir = tempfile::tempdir().unwrap();
    let from = dir.path().join("from");
    write(&from.join("skills/shared/SKILL.md"), "skill");
    write(&from.join("notes.txt"), "notes");
    fs::create_dir_all(from.join("engines")).unwrap();
    std::os::unix::fs::symlink("../skills/shared", from.join("engines/linked-skill")).unwrap();
    std::os::unix::fs::symlink("../notes.txt", from.join("engines/linked-notes")).unwrap();
    std::os::unix::fs::symlink("../missing", from.join("engines/dangling")).unwrap();
    let to = dir.path().join("to");

    copy_tree(&from, &to).unwrap();

    for (link, target) in [
        ("linked-skill", "../skills/shared"),
        ("linked-notes", "../notes.txt"),
        ("dangling", "../missing"),
    ] {
        assert_eq!(
            fs::read_link(to.join("engines").join(link)).unwrap(),
            Path::new(target)
        );
    }
    assert_eq!(read(&to.join("engines/linked-skill/SKILL.md")), "skill");
}
