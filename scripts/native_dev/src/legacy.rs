//! Adopting a hand-deployed Forge into the dev installation, once.
//!
//! Before the dev installation ran its own Forge service, a Forge was
//! deployed by hand: `forge-host` and `forge` from pinned Nix store paths
//! (kept alive by GC roots in `<state>/nix-roots`), a hand-written
//! `artisan-forge.service` user unit, and its home in
//! `$XDG_STATE_HOME/artisan-forge` (database, credentials, custody, managed
//! engines). Adoption moves that Forge into the installation so the new
//! service continues it:
//!
//! - the old unit is stopped and disabled first, and custody proves no Forge
//!   still owns the old home;
//! - the database, its WAL and shared memory files, and the credentials are
//!   copied to `<state>/adopted-backup` before anything moves;
//! - the database is checkpointed, then the database, credentials (the host
//!   identity every registered Editor trusts), custody, model catalog, and
//!   the whole engine toolchain move into the installation's layout;
//! - the GC roots are removed, and the unit, its drop-ins, and its saved
//!   copies move into the backup;
//! - `<state>/ADOPTED.json` marks the old home as a backup.
//!
//! Every step checks its own outcome, so an interrupted adoption resumes and
//! a finished one is a no-op. An item present at both its source and its
//! destination is a conflict that stops adoption untouched. A machine
//! without the old layout adopts nothing.

use std::{
    fs,
    path::{Path, PathBuf},
};

use artisan_editor_cli::service::{OWNER_KEY, Systemctl};

use crate::{error::DevError, paths::DevPaths};

/// Name of the hand-written unit.
pub const LEGACY_UNIT: &str = "artisan-forge.service";

/// Marker left in an adopted home.
pub const ADOPTED_MARKER: &str = "ADOPTED.json";

/// Backup directory inside an adopted home.
pub const BACKUP_DIRECTORY: &str = "adopted-backup";

/// Where a hand-deployed Forge lives.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LegacyForge {
    /// The old Forge home.
    pub state: PathBuf,
    /// The user unit directory holding the hand-written unit.
    pub unit_directory: PathBuf,
}

impl LegacyForge {
    /// The documented location: `$XDG_STATE_HOME/artisan-forge` (default
    /// `~/.local/state`) and `$XDG_CONFIG_HOME/systemd/user`.
    #[must_use]
    pub fn for_user(home: &Path, state_home: Option<&Path>, config_home: Option<&Path>) -> Self {
        Self {
            state: state_home
                .map_or_else(|| home.join(".local").join("state"), Path::to_path_buf)
                .join("artisan-forge"),
            unit_directory: config_home
                .map_or_else(|| home.join(".config"), Path::to_path_buf)
                .join("systemd")
                .join("user"),
        }
    }

    fn unit_path(&self) -> PathBuf {
        self.unit_directory.join(LEGACY_UNIT)
    }

    fn backup(&self) -> PathBuf {
        self.state.join(BACKUP_DIRECTORY)
    }

    /// Whether the unit of this name was written by hand rather than by an
    /// installation (which marks its units with [`OWNER_KEY`]).
    fn has_hand_written_unit(&self) -> bool {
        fs::read_to_string(self.unit_path())
            .is_ok_and(|text| !text.lines().any(|line| line.starts_with(OWNER_KEY)))
    }

    fn has_unadopted_home(&self) -> bool {
        self.state.is_dir() && !self.state.join(ADOPTED_MARKER).exists()
    }

    /// Whether anything remains to adopt.
    #[must_use]
    pub fn pending(&self) -> bool {
        self.has_hand_written_unit() || self.has_unadopted_home()
    }
}

/// What one adoption did.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Adoption {
    /// Items moved into the installation, as `old -> new` descriptions.
    pub moved: Vec<String>,
    /// Whether the hand-written unit was stopped and retired.
    pub unit_retired: bool,
    /// GC roots removed.
    pub gc_roots_removed: usize,
    /// Where the backup is.
    pub backup: PathBuf,
}

/// Flushes a database's write-ahead log into the database file.
pub type Checkpoint<'a> = &'a dyn Fn(&Path) -> Result<(), DevError>;

/// Adopts `legacy` into the installation at `target`.
///
/// Returns `None` when nothing is left to adopt.
///
/// # Errors
///
/// Returns [`DevError::Stage`] when a Forge still owns the old home, an item
/// exists at both its source and destination, or a step fails; completed
/// steps stay done and the next run resumes.
pub fn adopt(
    legacy: &LegacyForge,
    target: &DevPaths,
    systemctl: &dyn Systemctl,
    checkpoint: Checkpoint<'_>,
) -> Result<Option<Adoption>, DevError> {
    if !legacy.pending() {
        return Ok(None);
    }
    let mut adoption = Adoption {
        backup: legacy.backup(),
        ..Adoption::default()
    };
    let unit = legacy.has_hand_written_unit();
    if unit {
        // A unit that is not loaded or already stopped fails these quietly;
        // custody below is what proves the old Forge is gone.
        let _ = systemctl.run(&["stop", LEGACY_UNIT]);
        let _ = systemctl.run(&["disable", LEGACY_UNIT]);
    }
    if legacy.has_unadopted_home() {
        adopt_home(legacy, target, checkpoint, &mut adoption)?;
    }
    if unit {
        retire_unit(legacy)?;
        let _ = systemctl.run(&["daemon-reload"]);
        adoption.unit_retired = true;
    }
    Ok(Some(adoption))
}

fn adopt_home(
    legacy: &LegacyForge,
    target: &DevPaths,
    checkpoint: Checkpoint<'_>,
    adoption: &mut Adoption,
) -> Result<(), DevError> {
    let state = &legacy.state;
    let _custody = hold_custody(&state.join("custody"))?;
    back_up(legacy)?;
    let database = state.join("forge.db");
    if database.is_file() {
        checkpoint(&database)?;
    }
    let data = target.home.join("data");
    let target_database = target.database_path();
    let sidecar = |suffix: &str| {
        let mut name = target_database.as_os_str().to_owned();
        name.push(suffix);
        PathBuf::from(name)
    };
    let items = [
        (state.join("forge.db"), target_database.clone()),
        (state.join("forge.db-wal"), sidecar("-wal")),
        (state.join("forge.db-shm"), sidecar("-shm")),
        (state.join("credentials"), target.home.join("credentials")),
        (state.join("toolchain"), data.join("toolchain")),
        (
            state.join("model-catalog.json"),
            data.join("model-catalog.json"),
        ),
        (state.join("custody"), target.custody_path()),
    ];
    for (source, destination) in &items {
        if source.exists() && destination.exists() {
            return Err(adopt_error(format!(
                "{} and {} both exist; move one aside and rerun",
                source.display(),
                destination.display()
            )));
        }
    }
    for (source, destination) in items {
        if !source.exists() {
            continue;
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(io_error(parent))?;
        }
        fs::rename(&source, &destination).map_err(io_error(&source))?;
        adoption
            .moved
            .push(format!("{} -> {}", source.display(), destination.display()));
    }
    adoption.gc_roots_removed = remove_gc_roots(&state.join("nix-roots"))?;
    let marker = serde_json::json!({
        "adopted_into": target.home,
        "backup": legacy.backup(),
        "moved": adoption.moved,
    });
    fs::write(
        state.join(ADOPTED_MARKER),
        serde_json::to_vec_pretty(&marker).unwrap_or_default(),
    )
    .map_err(io_error(state))
}

/// Proves no Forge owns the old home: a live Forge holds its custody file's
/// lock for its whole lifetime.
fn hold_custody(custody: &Path) -> Result<Option<fs::File>, DevError> {
    let file = match fs::OpenOptions::new().read(true).write(true).open(custody) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error(custody)(error)),
    };
    fs2::FileExt::try_lock_exclusive(&file).map_err(|_| {
        adopt_error(format!(
            "a Forge still owns {}; stop it and rerun",
            custody.parent().unwrap_or(custody).display()
        ))
    })?;
    Ok(Some(file))
}

/// Copies the database files and credentials into the backup once.
fn back_up(legacy: &LegacyForge) -> Result<(), DevError> {
    let backup = legacy.backup();
    if backup.join("complete").exists() {
        return Ok(());
    }
    fs::create_dir_all(&backup).map_err(io_error(&backup))?;
    for name in ["forge.db", "forge.db-wal", "forge.db-shm"] {
        let source = legacy.state.join(name);
        if source.is_file() {
            fs::copy(&source, backup.join(name)).map_err(io_error(&source))?;
        }
    }
    copy_tree(
        &legacy.state.join("credentials"),
        &backup.join("credentials"),
    )?;
    fs::write(backup.join("complete"), b"").map_err(io_error(&backup))
}

fn copy_tree(source: &Path, destination: &Path) -> Result<(), DevError> {
    if !source.is_dir() {
        return Ok(());
    }
    fs::create_dir_all(destination).map_err(io_error(destination))?;
    for entry in fs::read_dir(source).map_err(io_error(source))? {
        let entry = entry.map_err(io_error(source))?;
        let kind = entry.file_type().map_err(io_error(&entry.path()))?;
        if kind.is_dir() {
            copy_tree(&entry.path(), &destination.join(entry.file_name()))?;
        } else if kind.is_file() {
            fs::copy(entry.path(), destination.join(entry.file_name()))
                .map_err(io_error(&entry.path()))?;
        }
    }
    Ok(())
}

/// Removes the GC root symlinks that kept the hand-deployed store paths.
fn remove_gc_roots(directory: &Path) -> Result<usize, DevError> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Ok(0);
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        if entry.file_type().is_ok_and(|kind| kind.is_symlink()) {
            fs::remove_file(entry.path()).map_err(io_error(&entry.path()))?;
            removed += 1;
        }
    }
    let _ = fs::remove_dir(directory);
    Ok(removed)
}

/// Moves the hand-written unit, its drop-in directory, and its saved copies
/// (`artisan-forge.service.*`) into the backup.
fn retire_unit(legacy: &LegacyForge) -> Result<(), DevError> {
    let destination = legacy.backup().join("systemd");
    fs::create_dir_all(&destination).map_err(io_error(&destination))?;
    let entries = fs::read_dir(&legacy.unit_directory).map_err(io_error(&legacy.unit_directory))?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(text) = name.to_str() else {
            continue;
        };
        if text == LEGACY_UNIT || text.starts_with(&format!("{LEGACY_UNIT}.")) {
            fs::rename(entry.path(), destination.join(&name)).map_err(io_error(&entry.path()))?;
        }
    }
    let wanted = legacy
        .unit_directory
        .join("default.target.wants")
        .join(LEGACY_UNIT);
    if wanted.symlink_metadata().is_ok() {
        fs::remove_file(&wanted).map_err(io_error(&wanted))?;
    }
    Ok(())
}

fn adopt_error(reason: String) -> DevError {
    DevError::Stage {
        stage: "adopt",
        reason,
    }
}

fn io_error(path: &Path) -> impl FnOnce(std::io::Error) -> DevError + '_ {
    move |error| adopt_error(format!("{}: {error}", path.display()))
}

/// The seam over `nix profile`, so the cleanup is testable offline.
pub trait NixProfile {
    /// `nix profile list --json`.
    ///
    /// # Errors
    ///
    /// Returns [`DevError::Stage`] when the profile cannot be listed.
    fn list(&self) -> Result<String, DevError>;
    /// `nix profile remove NAME`.
    ///
    /// # Errors
    ///
    /// Returns [`DevError::Stage`] when Nix refuses.
    fn remove(&self, name: &str) -> Result<(), DevError>;
}

/// The user's default Nix profile.
#[derive(Clone, Copy, Debug, Default)]
pub struct UserNixProfile;

impl NixProfile for UserNixProfile {
    fn list(&self) -> Result<String, DevError> {
        let output = std::process::Command::new("nix")
            .args(["profile", "list", "--json"])
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|error| adopt_error(format!("cannot run nix: {error}")))?;
        if !output.status.success() {
            return Err(adopt_error("nix profile list failed".to_owned()));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    fn remove(&self, name: &str) -> Result<(), DevError> {
        let status = std::process::Command::new("nix")
            .args(["profile", "remove", name])
            .stdin(std::process::Stdio::null())
            .status()
            .map_err(|error| adopt_error(format!("cannot run nix: {error}")))?;
        if status.success() {
            Ok(())
        } else {
            Err(adopt_error(format!("nix profile remove {name} failed")))
        }
    }
}

/// Names of profile elements that put an Artisan `ae` on the PATH: an
/// element named `artisan…` whose store path has `bin/ae`.
///
/// # Errors
///
/// Returns [`DevError::Stage`] for output that is not a profile listing.
pub fn artisan_ae_elements(listing: &str) -> Result<Vec<String>, DevError> {
    let document: serde_json::Value = serde_json::from_str(listing)
        .map_err(|error| adopt_error(format!("unreadable nix profile listing: {error}")))?;
    let elements = document
        .get("elements")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| adopt_error("the nix profile listing has no elements".to_owned()))?;
    Ok(elements
        .iter()
        .filter(|(name, _)| name.starts_with("artisan"))
        .filter(|(_, element)| {
            element
                .get("storePaths")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|paths| {
                    paths
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .any(|path| Path::new(path).join("bin").join("ae").exists())
                })
        })
        .map(|(name, _)| name.clone())
        .collect())
}

/// Removes every Artisan `ae` element from `profile`, returning their names.
///
/// # Errors
///
/// Returns [`DevError::Stage`] when the profile cannot be listed or changed.
pub fn remove_profile_ae(profile: &dyn NixProfile) -> Result<Vec<String>, DevError> {
    let names = artisan_ae_elements(&profile.list()?)?;
    for name in &names {
        profile.remove(name)?;
    }
    Ok(names)
}

/// Flushes the WAL of the SQLite database at `database` into it, so the
/// database file alone is the complete Forge state.
///
/// # Errors
///
/// Returns [`DevError::Stage`] when the database cannot be opened or
/// checkpointed.
pub fn checkpoint_database(database: &Path) -> Result<(), DevError> {
    use sea_orm::ConnectionTrait as _;

    let failed =
        |reason: String| adopt_error(format!("checkpoint {}: {reason}", database.display()));
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| failed(error.to_string()))?
        .block_on(async {
            let connection = artisan_database::connect(
                artisan_database::SqliteConfig::file(database)
                    .min_connections(1)
                    .max_connections(1)
                    .sqlx_logging(false),
            )
            .await
            .map_err(|error| failed(error.to_string()))?;
            connection
                .execute_unprepared("PRAGMA wal_checkpoint(TRUNCATE)")
                .await
                .map_err(|error| failed(error.to_string()))?;
            connection
                .close()
                .await
                .map_err(|error| failed(error.to_string()))
        })
}
