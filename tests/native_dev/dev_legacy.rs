//! Adopting the hand-deployed Forge (old home, hand-written unit, GC roots,
//! hand-added `ae`) into the dev installation, from a fixture of the old
//! layout, through fake `systemctl` and `nix profile` seams.

#![cfg(unix)]

use std::{
    cell::RefCell,
    fs,
    os::unix::fs::symlink,
    path::{Path, PathBuf},
};

use artisan_editor_cli::{
    Result as CliResult,
    service::{Systemctl, SystemctlOutput},
};
use native_dev::{
    DevError, DevPaths,
    legacy::{
        ADOPTED_MARKER, BACKUP_DIRECTORY, LEGACY_UNIT, LegacyForge, NixProfile, adopt,
        artisan_ae_elements, checkpoint_database, remove_profile_ae,
    },
};

#[derive(Default)]
struct FakeSystemctl {
    calls: RefCell<Vec<String>>,
}

impl Systemctl for FakeSystemctl {
    fn run(&self, arguments: &[&str]) -> CliResult<SystemctlOutput> {
        self.calls.borrow_mut().push(arguments.join(" "));
        Ok(SystemctlOutput {
            success: true,
            stdout: String::new(),
        })
    }
}

struct Fixture {
    _scratch: tempfile::TempDir,
    legacy: LegacyForge,
    target: DevPaths,
}

fn write(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().expect("parent")).expect("directory");
    fs::write(path, contents).expect("fixture file");
}

/// The layout `scripts/install_forge_host.py` left behind.
fn fixture() -> Fixture {
    let scratch = tempfile::tempdir().expect("scratch");
    let home = scratch.path().join("home");
    let legacy = LegacyForge::for_user(&home, None, None);
    let state = &legacy.state;
    write(&state.join("forge.db"), "database");
    write(&state.join("forge.db-wal"), "write-ahead log");
    write(&state.join("forge.db-shm"), "shared memory");
    write(
        &state.join("credentials/manifest.json"),
        "credential manifest",
    );
    write(&state.join("credentials/localhost-leaf.der"), "certificate");
    write(&state.join("custody"), "");
    write(&state.join("model-catalog.json"), "catalog");
    write(&state.join("host.json"), "old invitation");
    write(&state.join("readiness.json"), "old readiness");
    write(&state.join("backups/before-x.db"), "manual backup");
    write(&state.join("toolchain/claude/state.json"), "claude state");
    write(
        &state.join("toolchain/claude/versions/generation-1/claude"),
        "claude binary",
    );
    write(
        &state.join("toolchain/claude/home/.claude/.credentials.json"),
        "login",
    );
    write(&state.join("toolchain/grok/trust.json"), "grok trust");
    write(
        &state.join("toolchain/codex/install-failure.json"),
        "failure",
    );
    fs::create_dir_all(state.join("nix-roots")).expect("roots");
    for root in ["ae", "forge", "forge-host"] {
        symlink(
            format!("/nix/store/0000-artisan-{root}"),
            state.join("nix-roots").join(root),
        )
        .expect("gc root");
    }
    let units = &legacy.unit_directory;
    write(
        &units.join(LEGACY_UNIT),
        "[Service]\nExecStart=\"/nix/store/k1-artisan-forge-host/bin/forge-host\" \"--home\" \"/x\"\n",
    );
    write(&units.join(format!("{LEGACY_UNIT}.previous")), "previous");
    write(
        &units.join(format!("{LEGACY_UNIT}.before-sidebar-1")),
        "saved",
    );
    write(
        &units.join(format!("{LEGACY_UNIT}.d/codex.conf")),
        "drop-in",
    );
    write(&units.join("other.service"), "[Service]\n");
    fs::create_dir_all(units.join("default.target.wants")).expect("wants");
    symlink(
        units.join(LEGACY_UNIT),
        units.join("default.target.wants").join(LEGACY_UNIT),
    )
    .expect("enablement");
    let target = DevPaths::new(&scratch.path().join("data/Artisan Street Dev")).expect("root");
    Fixture {
        _scratch: scratch,
        legacy,
        target,
    }
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|_| panic!("{} exists", path.display()))
}

#[test]
fn the_old_forge_moves_into_the_installation_with_a_backup() {
    let fixture = fixture();
    let systemctl = FakeSystemctl::default();
    let seen: RefCell<Vec<PathBuf>> = RefCell::new(Vec::new());
    let checkpoint = |path: &Path| -> Result<(), DevError> {
        seen.borrow_mut().push(path.to_path_buf());
        Ok(())
    };
    let adoption = adopt(&fixture.legacy, &fixture.target, &systemctl, &checkpoint)
        .expect("adopts")
        .expect("something to adopt");

    // The old Forge was stopped before anything moved, and its unit retired.
    assert_eq!(
        systemctl.calls.borrow().as_slice(),
        [
            "stop artisan-forge.service",
            "disable artisan-forge.service",
            "daemon-reload"
        ]
    );
    assert_eq!(
        seen.borrow().as_slice(),
        [fixture.legacy.state.join("forge.db")]
    );
    assert!(adoption.unit_retired);
    assert_eq!(adoption.gc_roots_removed, 3);

    let root = &fixture.target.home;
    assert_eq!(read(&fixture.target.database_path()), "database");
    assert_eq!(
        read(&root.join("data/forge.sqlite3-wal")),
        "write-ahead log"
    );
    assert_eq!(read(&root.join("data/forge.sqlite3-shm")), "shared memory");
    assert_eq!(
        read(&root.join("credentials/manifest.json")),
        "credential manifest"
    );
    assert_eq!(
        read(&root.join("credentials/localhost-leaf.der")),
        "certificate"
    );
    assert_eq!(read(&fixture.target.custody_path()), "");
    assert_eq!(read(&root.join("data/model-catalog.json")), "catalog");
    // The whole toolchain moves: generations, engine homes (logins), trust
    // records, and install failures.
    for (path, contents) in [
        ("data/toolchain/claude/state.json", "claude state"),
        (
            "data/toolchain/claude/versions/generation-1/claude",
            "claude binary",
        ),
        (
            "data/toolchain/claude/home/.claude/.credentials.json",
            "login",
        ),
        ("data/toolchain/grok/trust.json", "grok trust"),
        ("data/toolchain/codex/install-failure.json", "failure"),
    ] {
        assert_eq!(read(&root.join(path)), contents, "{path}");
    }

    let state = &fixture.legacy.state;
    let backup = state.join(BACKUP_DIRECTORY);
    assert_eq!(adoption.backup, backup);
    assert_eq!(read(&backup.join("forge.db")), "database");
    assert_eq!(read(&backup.join("forge.db-wal")), "write-ahead log");
    assert_eq!(
        read(&backup.join("credentials/manifest.json")),
        "credential manifest"
    );
    assert_eq!(
        read(&backup.join("systemd").join(LEGACY_UNIT))
            .lines()
            .count(),
        2
    );
    assert_eq!(
        read(&backup.join(format!("systemd/{LEGACY_UNIT}.previous"))),
        "previous"
    );
    assert_eq!(
        read(&backup.join(format!("systemd/{LEGACY_UNIT}.before-sidebar-1"))),
        "saved"
    );
    assert_eq!(
        read(&backup.join(format!("systemd/{LEGACY_UNIT}.d/codex.conf"))),
        "drop-in"
    );

    // The old home is only a backup marker now; unrelated files stay.
    assert!(state.join(ADOPTED_MARKER).is_file());
    assert!(!state.join("forge.db").exists());
    assert!(!state.join("credentials").exists());
    assert!(!state.join("nix-roots").exists());
    assert_eq!(read(&state.join("backups/before-x.db")), "manual backup");
    let units = &fixture.legacy.unit_directory;
    assert!(!units.join(LEGACY_UNIT).exists());
    assert!(!units.join(format!("{LEGACY_UNIT}.d")).exists());
    assert!(
        units
            .join("default.target.wants")
            .join(LEGACY_UNIT)
            .symlink_metadata()
            .is_err()
    );
    assert!(units.join("other.service").is_file());
}

#[test]
fn adoption_is_idempotent() {
    let fixture = fixture();
    let checkpoint = |_: &Path| -> Result<(), DevError> { Ok(()) };
    adopt(
        &fixture.legacy,
        &fixture.target,
        &FakeSystemctl::default(),
        &checkpoint,
    )
    .expect("first adoption");
    assert!(!fixture.legacy.pending());
    let systemctl = FakeSystemctl::default();
    assert!(
        adopt(&fixture.legacy, &fixture.target, &systemctl, &checkpoint)
            .expect("second run")
            .is_none()
    );
    assert!(systemctl.calls.borrow().is_empty());
    assert_eq!(read(&fixture.target.database_path()), "database");
}

#[test]
fn an_interrupted_adoption_resumes_and_a_conflict_moves_nothing() {
    let fixture = fixture();
    let checkpoint = |_: &Path| -> Result<(), DevError> { Ok(()) };
    // Interrupted after the toolchain moved: the rest resumes.
    let root = &fixture.target.home;
    fs::create_dir_all(root.join("data")).expect("data");
    fs::rename(
        fixture.legacy.state.join("toolchain"),
        root.join("data/toolchain"),
    )
    .expect("partial move");
    adopt(
        &fixture.legacy,
        &fixture.target,
        &FakeSystemctl::default(),
        &checkpoint,
    )
    .expect("resumes");
    assert_eq!(read(&fixture.target.database_path()), "database");
    assert_eq!(
        read(&root.join("data/toolchain/grok/trust.json")),
        "grok trust"
    );

    // A database at both ends is a conflict: nothing moves.
    let conflicting = self::fixture();
    write(&conflicting.target.database_path(), "newer database");
    let error = adopt(
        &conflicting.legacy,
        &conflicting.target,
        &FakeSystemctl::default(),
        &checkpoint,
    )
    .expect_err("conflict");
    assert!(error.to_string().contains("both exist"), "{error}");
    assert_eq!(read(&conflicting.legacy.state.join("forge.db")), "database");
    assert!(conflicting.legacy.state.join("credentials").is_dir());
    assert!(!conflicting.legacy.state.join(ADOPTED_MARKER).exists());
}

#[test]
fn a_forge_still_holding_custody_stops_adoption() {
    let fixture = fixture();
    let custody = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(fixture.legacy.state.join("custody"))
        .expect("custody");
    fs2::FileExt::try_lock_exclusive(&custody).expect("a live Forge holds custody");
    let checkpoint = |_: &Path| -> Result<(), DevError> { Ok(()) };
    let error = adopt(
        &fixture.legacy,
        &fixture.target,
        &FakeSystemctl::default(),
        &checkpoint,
    )
    .expect_err("refused");
    assert!(error.to_string().contains("still owns"), "{error}");
    assert_eq!(read(&fixture.legacy.state.join("forge.db")), "database");
}

#[test]
fn a_fresh_machine_and_a_product_unit_adopt_nothing() {
    let scratch = tempfile::tempdir().expect("scratch");
    let legacy = LegacyForge::for_user(&scratch.path().join("home"), None, None);
    let target = DevPaths::new(&scratch.path().join("Artisan Street Dev")).expect("root");
    let systemctl = FakeSystemctl::default();
    let checkpoint = |_: &Path| -> Result<(), DevError> { panic!("nothing to checkpoint") };
    assert!(
        adopt(&legacy, &target, &systemctl, &checkpoint)
            .expect("nothing")
            .is_none()
    );
    // A unit the product wrote under the same name is not hand-written.
    write(
        &legacy.unit_directory.join(LEGACY_UNIT),
        "[Unit]\nX-ArtisanInstallRoot=/home/ada/.local/share/Artisan Street\n",
    );
    assert!(!legacy.pending());
    assert!(systemctl.calls.borrow().is_empty());
}

#[test]
fn xdg_directories_locate_the_old_forge() {
    let legacy = LegacyForge::for_user(
        Path::new("/home/ada"),
        Some(Path::new("/state")),
        Some(Path::new("/config")),
    );
    assert_eq!(legacy.state, Path::new("/state/artisan-forge"));
    assert_eq!(legacy.unit_directory, Path::new("/config/systemd/user"));
    let defaults = LegacyForge::for_user(Path::new("/home/ada"), None, None);
    assert_eq!(
        defaults.state,
        Path::new("/home/ada/.local/state/artisan-forge")
    );
}

#[test]
fn the_checkpoint_folds_the_write_ahead_log_into_the_database() {
    use sea_orm::ConnectionTrait as _;

    let scratch = tempfile::tempdir().expect("scratch");
    let database = scratch.path().join("forge.db");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    // A writer that stays open keeps its committed pages in the WAL.
    let writer = runtime.block_on(async {
        let connection = artisan_database::connect(
            artisan_database::SqliteConfig::file(&database)
                .min_connections(1)
                .max_connections(1),
        )
        .await
        .expect("database");
        connection
            .execute_unprepared(
                "CREATE TABLE fixture (value TEXT); INSERT INTO fixture VALUES ('kept');",
            )
            .await
            .expect("write");
        connection
    });
    let wal = scratch.path().join("forge.db-wal");
    assert!(fs::metadata(&wal).expect("wal").len() > 0);
    checkpoint_database(&database).expect("checkpoint");
    assert_eq!(fs::metadata(&wal).map_or(0, |metadata| metadata.len()), 0);
    runtime.block_on(async {
        let rows = writer
            .query_all_raw(sea_orm::Statement::from_string(
                sea_orm::DbBackend::Sqlite,
                "SELECT value FROM fixture",
            ))
            .await
            .expect("read back");
        assert_eq!(rows.len(), 1);
        writer.close().await.expect("close");
    });
}

struct FakeProfile {
    listing: String,
    removed: RefCell<Vec<String>>,
}

impl NixProfile for FakeProfile {
    fn list(&self) -> Result<String, DevError> {
        Ok(self.listing.clone())
    }

    fn remove(&self, name: &str) -> Result<(), DevError> {
        self.removed.borrow_mut().push(name.to_owned());
        Ok(())
    }
}

#[test]
fn only_hand_added_artisan_ae_leaves_the_nix_profile() {
    let scratch = tempfile::tempdir().expect("scratch");
    let store = |name: &str, binary: &str| {
        let path = scratch.path().join(name);
        write(&path.join("bin").join(binary), "binary");
        path
    };
    let ae = store("2ldf-artisan-ae-release-0.0.0", "ae");
    let forge = store("z9y7-artisan-forge-release-0.0.0", "forge");
    let other = store("aaaa-other-ae-0.1", "ae");
    let listing = serde_json::json!({
        "version": 3,
        "elements": {
            "artisan-ae-release": { "active": true, "storePaths": [ae] },
            "artisan-forge-release": { "active": true, "storePaths": [forge] },
            "other-ae": { "active": true, "storePaths": [other] },
        }
    })
    .to_string();
    assert_eq!(
        artisan_ae_elements(&listing).expect("listing"),
        ["artisan-ae-release"]
    );
    let profile = FakeProfile {
        listing,
        removed: RefCell::new(Vec::new()),
    };
    assert_eq!(
        remove_profile_ae(&profile).expect("removed"),
        ["artisan-ae-release"]
    );
    assert_eq!(profile.removed.borrow().as_slice(), ["artisan-ae-release"]);
    assert!(artisan_ae_elements("[]").is_err());
}
