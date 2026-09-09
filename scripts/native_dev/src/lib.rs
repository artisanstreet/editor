//! Native development installation orchestration.
//!
//! `bazel run //:dev` builds the Editor and Forge binaries through the
//! authoritative Bazel graph, stages them into an isolated development
//! installation under `<workspace>/.dist/dev`, provisions that home through
//! the existing CLI custody APIs (installation manifest, payload integrity,
//! Forge credentials, native instance configuration), and launches the
//! staged Editor. The Editor then performs its unchanged shipping startup:
//! it discovers the dev home through `ARTISAN_HOME`, verifies the staged
//! payload, starts its newly owned Forge through
//! [`artisan_editor_cli::process::start_owned`], and connects over
//! authenticated QUIC. This crate invents no transport, handshake, or
//! credential flow; it only stages files and spawns the Editor.
//!
//! The real installed application is never touched: every path lives under
//! the dev directory, and repeat invocations preserve the dev database,
//! credentials, and instance identity.

#![forbid(unsafe_code)]

use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use artisan_editor_cli::{
    credentials::{self, ForgeCredentialPaths},
    instance::{NativeInstanceConfig, NativeListenerConfig, NativeRunConfig, NativeRunConfigInput},
    manifest::InstallationManifest,
    payload::{self, PAYLOAD_MANIFEST_NAME},
    process::{self, ForgeReadinessStatus},
};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Active version selected for every development installation.
pub const DEV_VERSION: &str = "dev";

/// Home directory name inside the dev directory.
pub const DEV_HOME_NAME: &str = "home";

/// Environment variable pointing the staged Editor at the dev home.
///
/// This is the same variable the shipping CLI discovery honors; the dev
/// home is explicit, never a platform default, so the real installation is
/// unreachable from a staged Editor.
pub const DEV_HOME_ENV: &str = "ARTISAN_HOME";

/// Legacy dev escape hatch that must not leak into the staged Editor.
///
/// When set, the Editor would attach to a manually started Forge instead of
/// starting its owned Forge. The dev launcher strips it so every `//:dev`
/// run exercises the owned custody path.
pub const STRIPPED_DEV_HOME_ENV: &str = "ARTISAN_DEV_FORGE_HOME";

/// Companion override stripped alongside [`STRIPPED_DEV_HOME_ENV`].
pub const STRIPPED_DEV_READY_ENV: &str = "ARTISAN_DEV_FORGE_READY_FILE";

/// Bazel workspace directory marker honored for the default dev directory.
pub const WORKSPACE_ENV: &str = "BUILD_WORKSPACE_DIRECTORY";

/// Runfiles directory marker present under `bazel run`.
pub const RUNFILES_DIR_ENV: &str = "RUNFILES_DIR";

/// Runfiles manifest marker present under `bazel run` on Windows.
pub const RUNFILES_MANIFEST_ENV: &str = "RUNFILES_MANIFEST_FILE";

/// Dev-directory leaf holding the isolated installation.
pub const DIST_DEV_LEAF: &str = ".dist/dev";

/// Listener admission budget for a dev Forge, in milliseconds.
pub const DEV_ADMISSION_TIMEOUT_MS: u64 = 5_000;
/// Handshake budget for a dev Forge, in milliseconds.
pub const DEV_HANDSHAKE_TIMEOUT_MS: u64 = 5_000;
/// Per-request budget for a dev Forge, in milliseconds.
pub const DEV_REQUEST_TIMEOUT_MS: u64 = 30_000;
/// Drain budget for a dev Forge shutdown, in milliseconds.
pub const DEV_DRAIN_TIMEOUT_MS: u64 = 2_000;
/// Lifetime admission capacity for a dev Forge.
pub const DEV_ADMISSION_CAPACITY: u32 = 64;
/// Per-connection request capacity for a dev Forge.
pub const DEV_REQUESTS_PER_CONNECTION: u32 = 32;

/// Native-run claim lease for a dev Forge, in milliseconds.
pub const DEV_RUN_CLAIM_LEASE_MS: u64 = 30_000;
/// Native-run poll interval for a dev Forge, in milliseconds.
pub const DEV_RUN_POLL_INTERVAL_MS: u64 = 500;
/// Native-run retry backoff for a dev Forge, in milliseconds.
pub const DEV_RUN_RETRY_BACKOFF_MS: u64 = 1_000;
/// Native-run shutdown budget for a dev Forge, in milliseconds.
pub const DEV_RUN_SHUTDOWN_BUDGET_MS: u64 = 2_000;
/// Native-run queue capacity for a dev Forge.
pub const DEV_RUN_QUEUE_CAPACITY: u32 = 64;
/// Native-run command retry bound for a dev Forge.
pub const DEV_RUN_MAX_COMMAND_RETRIES: u32 = 3;
/// Native-run prompt delivery mode for a dev Forge.
pub const DEV_RUN_PROMPT_DELIVERY: &str = "queue";
/// Native-run stream threshold for a dev Forge.
pub const DEV_RUN_STREAM_AFTER: u64 = 0;

/// Bounded failure for one dev stage.
///
/// Diagnostics name stages and paths but never credential material: paths
/// are operational context, secrets never cross this boundary.
#[derive(Debug, Error)]
pub enum DevError {
    /// Command-line usage was invalid.
    #[error("invalid arguments: {reason}")]
    Usage {
        /// What was wrong with the invocation.
        reason: String,
    },
    /// A required path was not absolute.
    #[error("path must be absolute: {path}")]
    NotAbsolute {
        /// The offending path.
        path: PathBuf,
    },
    /// A staged binary could not be located.
    #[error("dev binary missing: {name} ({hint})")]
    BinaryMissing {
        /// Binary file name that was not found.
        name: String,
        /// Where the search looked.
        hint: String,
    },
    /// A dev stage failed with a bounded reason.
    #[error("stage {stage} failed: {reason}")]
    Stage {
        /// Stage that failed.
        stage: &'static str,
        /// Bounded human-readable reason.
        reason: String,
    },
    /// The payload gate rejected the staged version.
    #[error("staged payload is not verified: {issues}")]
    PayloadUnverified {
        /// Integrity findings from the existing verifier.
        issues: String,
    },
    /// A previous dev Forge still owns the dev home.
    #[error(
        "previous dev Forge still running with pid {pid}; close the previous dev session first"
    )]
    PreviousForgeRunning {
        /// Live Forge process identity from the readiness receipt.
        pid: u32,
    },
    /// The staged Editor exited abnormally.
    #[error("staged editor exited with {status}")]
    EditorStatus {
        /// Exit status text.
        status: String,
    },
}

/// What the argument parser decided.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Action {
    /// Print usage text.
    Help,
    /// Run the dev stages.
    Run(DevArgs),
}

/// Parsed `dev` invocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DevArgs {
    /// Explicit dev directory; defaults to `<workspace>/.dist/dev`.
    pub dev_dir: Option<PathBuf>,
    /// Explicit directory holding prebuilt `ae`/`editor`/`forge`/`installer`
    /// binaries; defaults to the Bazel runfiles search.
    pub bin_dir: Option<PathBuf>,
    /// Stage and provision only; do not launch the Editor.
    pub stage_only: bool,
}

impl DevArgs {
    /// Parses one argv slice (without the program name).
    ///
    /// # Errors
    ///
    /// Returns [`DevError::Usage`] for unknown flags or missing values.
    pub fn parse(argv: &[OsString]) -> Result<Action, DevError> {
        let mut args = Self {
            dev_dir: None,
            bin_dir: None,
            stage_only: false,
        };
        let mut rest = argv.iter();
        while let Some(flag) = rest.next() {
            let flag = flag.to_string_lossy();
            match flag.as_ref() {
                "-h" | "--help" => return Ok(Action::Help),
                "--stage-only" => args.stage_only = true,
                "--dev-dir" => {
                    let value = rest.next().ok_or_else(|| DevError::Usage {
                        reason: "--dev-dir requires a path value".to_owned(),
                    })?;
                    args.dev_dir = Some(PathBuf::from(value));
                }
                "--bin-dir" => {
                    let value = rest.next().ok_or_else(|| DevError::Usage {
                        reason: "--bin-dir requires a path value".to_owned(),
                    })?;
                    args.bin_dir = Some(PathBuf::from(value));
                }
                unknown => {
                    return Err(DevError::Usage {
                        reason: format!("unknown flag `{unknown}`"),
                    });
                }
            }
        }
        Ok(Action::Run(args))
    }
}

/// Short usage text for `--help`.
#[must_use]
pub fn usage() -> &'static str {
    "usage: dev [--dev-dir PATH] [--bin-dir PATH] [--stage-only]\n\
     \n\
     Stage Bazel-built Editor and Forge binaries into <workspace>/.dist/dev,\n\
     provision the isolated dev home through the existing CLI custody APIs,\n\
     and launch the staged Editor on its newly owned Forge.\n\
     \n\
     --dev-dir PATH  isolated installation root (default: <workspace>/.dist/dev)\n\
     --bin-dir PATH  directory with prebuilt ae/editor/forge/installer binaries\n\
     --stage-only    stage and provision without launching the Editor"
}

/// Every path owned by one development installation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DevPaths {
    /// Isolated installation root (`<dev-dir>` itself is not the home).
    pub dev_dir: PathBuf,
    /// Forge home the staged Editor discovers through `ARTISAN_HOME`.
    pub home: PathBuf,
    /// Staged version root (`<home>/versions/dev`).
    pub version_root: PathBuf,
    /// Staged binary directory (`<version-root>/bin`).
    pub version_bin: PathBuf,
    /// Installation manifest (`<home>/installation.json`).
    pub manifest_path: PathBuf,
    /// Permanent launcher (`<home>/bin/ae[.exe]`).
    pub permanent_ae: PathBuf,
}

impl DevPaths {
    /// Derives every owned path from one absolute dev directory.
    ///
    /// # Errors
    ///
    /// Returns [`DevError::NotAbsolute`] when `dev_dir` is not absolute.
    pub fn new(dev_dir: &Path) -> Result<Self, DevError> {
        if !dev_dir.is_absolute() {
            return Err(DevError::NotAbsolute {
                path: dev_dir.to_path_buf(),
            });
        }
        let home = dev_dir.join(DEV_HOME_NAME);
        let version_root = home.join("versions").join(DEV_VERSION);
        Ok(Self {
            dev_dir: dev_dir.to_path_buf(),
            home: home.clone(),
            version_root: version_root.clone(),
            version_bin: version_root.join("bin"),
            manifest_path: home.join("installation.json"),
            permanent_ae: home.join("bin").join(exe_name("ae")),
        })
    }

    /// Native instance database path owned by this home.
    #[must_use]
    pub fn database_path(&self) -> PathBuf {
        self.home.join("data").join("forge.sqlite3")
    }

    /// Native process-custody lock path owned by this home.
    #[must_use]
    pub fn custody_path(&self) -> PathBuf {
        self.home.join("custody").join("forge.lock")
    }

    /// Forge readiness receipt path owned by this home.
    #[must_use]
    pub fn readiness_path(&self) -> PathBuf {
        self.home.join("readiness").join("forge.json")
    }
}

/// Resolves the dev directory: explicit flag, then `<workspace>/.dist/dev`
/// from [`WORKSPACE_ENV`], then `<current-dir>/.dist/dev`.
#[must_use]
pub fn default_base_dir(workspace: Option<&Path>, current_dir: &Path) -> PathBuf {
    workspace.map_or_else(
        || current_dir.join(DIST_DEV_LEAF),
        |root| root.join(DIST_DEV_LEAF),
    )
}

/// Resolves and validates the effective dev directory.
///
/// # Errors
///
/// Returns [`DevError::NotAbsolute`] when the resolved directory is not
/// absolute.
pub fn resolve_dev_dir(explicit: Option<&Path>) -> Result<PathBuf, DevError> {
    let dev_dir = match explicit {
        Some(path) => path.to_path_buf(),
        None => {
            let workspace = std::env::var_os(WORKSPACE_ENV).map(PathBuf::from);
            let current = std::env::current_dir().map_err(|_| DevError::Stage {
                stage: "resolve",
                reason: "working directory is unavailable".to_owned(),
            })?;
            default_base_dir(workspace.as_deref(), &current)
        }
    };
    if !dev_dir.is_absolute() {
        return Err(DevError::NotAbsolute { path: dev_dir });
    }
    Ok(dev_dir)
}

/// Platform binary file name.
#[must_use]
pub fn exe_name(stem: &str) -> String {
    if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_owned()
    }
}

/// Located build outputs for the four payload binaries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinarySet {
    /// Staged `ae` build output.
    pub ae: PathBuf,
    /// Staged `editor` build output.
    pub editor: PathBuf,
    /// Staged `forge` build output.
    pub forge: PathBuf,
    /// Staged `installer` build output.
    pub installer: PathBuf,
}

impl BinarySet {
    /// Iterates `(payload-relative-name, source-path)` pairs.
    pub fn entries(&self) -> [(String, PathBuf); 4] {
        [
            (format!("bin/{}", exe_name("ae")), self.ae.clone()),
            (
                format!("bin/{}", exe_name("installer")),
                self.installer.clone(),
            ),
            (format!("bin/{}", exe_name("editor")), self.editor.clone()),
            (format!("bin/{}", exe_name("forge")), self.forge.clone()),
        ]
    }
}

/// Runfile-relative locations of the four binaries inside the workspace.
fn runfile_names() -> [(&'static str, &'static str); 4] {
    [
        ("ae", "modules/cli/ae"),
        ("installer", "modules/installer/installer"),
        ("editor", "modules/frontend/editor"),
        ("forge", "modules/backend/forge"),
    ]
}

/// Finds one runfile inside Bazel runfiles directory text.
///
/// The manifest format is `<runfile> <local-path>` per line; the first
/// ASCII space separates the name from the path.
#[must_use]
pub fn find_in_manifest(manifest: &str, runfile: &str) -> Option<PathBuf> {
    manifest.lines().find_map(|line| {
        let (name, path) = line.split_once(' ')?;
        if name.trim() == runfile {
            let trimmed = path.trim();
            (!trimmed.is_empty()).then(|| PathBuf::from(trimmed))
        } else {
            None
        }
    })
}

/// Candidate runfiles locations for one binary, in search order.
#[must_use]
pub fn runfiles_candidates(runfile: &str, runfiles_dir: &str) -> Vec<PathBuf> {
    let suffixed = if cfg!(windows) {
        format!("{runfile}.exe")
    } else {
        runfile.to_owned()
    };
    vec![
        PathBuf::from(runfiles_dir)
            .join("artisan_editor")
            .join(&suffixed),
        PathBuf::from(runfiles_dir).join(&suffixed),
    ]
}

/// Locates the four build outputs.
///
/// Search order: explicit `--bin-dir`, `$RUNFILES_DIR` (both legacy
/// workspace-prefixed and flat layouts), `$RUNFILES_MANIFEST_FILE`, then
/// the `bazel-bin` sibling layout relative to the current executable.
///
/// # Errors
///
/// Returns [`DevError::BinaryMissing`] naming every binary that could not
/// be located.
pub fn locate_binaries(explicit: Option<&Path>) -> Result<BinarySet, DevError> {
    if let Some(directory) = explicit {
        return locate_in_dir(directory);
    }
    let mut missing: Vec<String> = Vec::new();
    let mut found: BTreeMap<&str, PathBuf> = BTreeMap::new();
    if let Some(runfiles_dir) = std::env::var_os(RUNFILES_DIR_ENV) {
        let runfiles_dir = runfiles_dir.to_string_lossy().into_owned();
        for (stem, runfile) in runfile_names() {
            if found.contains_key(stem) {
                continue;
            }
            for candidate in runfiles_candidates(runfile, &runfiles_dir) {
                if candidate.is_file() {
                    found.insert(stem, candidate);
                    break;
                }
            }
        }
    }
    if found.len() < 4
        && let Some(manifest_path) = std::env::var_os(RUNFILES_MANIFEST_ENV)
        && let Ok(manifest) = fs::read_to_string(&manifest_path)
    {
        for (stem, runfile) in runfile_names() {
            if found.contains_key(stem) {
                continue;
            }
            let suffixed = if cfg!(windows) {
                format!("{runfile}.exe")
            } else {
                runfile.to_owned()
            };
            let plain = find_in_manifest(&manifest, &suffixed)
                .or_else(|| find_in_manifest(&manifest, runfile));
            if let Some(path) = plain.filter(|path| path.is_file()) {
                found.insert(stem, path);
            }
        }
    }
    if found.len() < 4
        && let Ok(exe) = std::env::current_exe()
        && let Some(exe_dir) = exe.parent()
    {
        for (stem, runfile) in runfile_names() {
            if found.contains_key(stem) {
                continue;
            }
            let candidate = exe_dir
                .join("../../")
                .join(runfile)
                .with_extension(exe_extension());
            if candidate.is_file() {
                found.insert(stem, candidate);
            }
        }
    }
    for (stem, _) in runfile_names() {
        if !found.contains_key(stem) {
            missing.push(exe_name(stem));
        }
    }
    if !missing.is_empty() {
        return Err(DevError::BinaryMissing {
            name: missing.join(", "),
            hint: "expected bazel run runfiles or --bin-dir".to_owned(),
        });
    }
    let mut found = found;
    let mut take = |stem: &str| {
        found
            .remove(stem)
            .unwrap_or_else(|| PathBuf::from(exe_name(stem)))
    };
    Ok(BinarySet {
        ae: take("ae"),
        editor: take("editor"),
        forge: take("forge"),
        installer: take("installer"),
    })
}

/// Extension used for sibling-layout probing.
fn exe_extension() -> &'static str {
    if cfg!(windows) { "exe" } else { "" }
}

/// Locates the four binaries inside one explicit directory.
///
/// # Errors
///
/// Returns [`DevError::BinaryMissing`] when any binary is absent.
pub fn locate_in_dir(directory: &Path) -> Result<BinarySet, DevError> {
    let mut missing = Vec::new();
    let mut get = |stem: &str| {
        let path = directory.join(exe_name(stem));
        if path.is_file() {
            Some(path)
        } else {
            missing.push(exe_name(stem));
            None
        }
    };
    let set = BinarySet {
        ae: get("ae").unwrap_or_default(),
        editor: get("editor").unwrap_or_default(),
        forge: get("forge").unwrap_or_default(),
        installer: get("installer").unwrap_or_default(),
    };
    if missing.is_empty() {
        Ok(set)
    } else {
        Err(DevError::BinaryMissing {
            name: missing.join(", "),
            hint: format!("--bin-dir {}", directory.display()),
        })
    }
}

/// Lowercase hex SHA-256 of one file.
///
/// # Errors
///
/// Returns [`DevError::Stage`] when the file cannot be read.
pub fn hash_file(path: &Path) -> Result<String, DevError> {
    let mut file = fs::File::open(path).map_err(|_| DevError::Stage {
        stage: "stage",
        reason: format!("cannot read {}", path.display()),
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|_| DevError::Stage {
            stage: "stage",
            reason: format!("cannot read {}", path.display()),
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex_encode(&hasher.finalize()))
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut text, byte| {
            use std::fmt::Write;
            let _ = write!(text, "{byte:02x}");
            text
        })
}

/// Stages one binary: copies only when the destination hash differs so
/// repeat invocations leave a running dev installation undisturbed.
///
/// Returns `true` when the destination was (re)written.
///
/// # Errors
///
/// Returns [`DevError::Stage`] when staging fails; the reason names the
/// likely cause (a locked destination means a previous dev session is still
/// running from it).
pub fn stage_one_binary(source: &Path, destination: &Path) -> Result<bool, DevError> {
    if destination.is_file() {
        let current = hash_file(destination)?;
        let incoming = hash_file(source)?;
        if current == incoming {
            return Ok(false);
        }
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|_| DevError::Stage {
            stage: "stage",
            reason: format!("cannot create {}", parent.display()),
        })?;
    }
    fs::copy(source, destination).map_err(|_| DevError::Stage {
        stage: "stage",
        reason: format!(
            "cannot stage {} (a previous dev Editor or Forge may still run from it)",
            destination.display()
        ),
    })?;
    Ok(true)
}

/// Stages all four binaries plus the permanent `ae` launcher.
///
/// # Errors
///
/// Returns [`DevError::Stage`] when any copy fails.
pub fn stage_binaries(set: &BinarySet, paths: &DevPaths) -> Result<Vec<(String, bool)>, DevError> {
    let mut staged = Vec::with_capacity(5);
    for (relative, source) in set.entries() {
        let written = stage_one_binary(&source, &paths.version_root.join(&relative))?;
        staged.push((relative, written));
    }
    let written = stage_one_binary(&set.ae, &paths.permanent_ae)?;
    staged.push((format!("bin/{}", exe_name("ae")), written));
    Ok(staged)
}

/// Writes one file atomically through a sibling temporary plus rename.
///
/// # Errors
///
/// Returns [`DevError::Stage`] when the write fails.
pub fn write_atomic(path: &Path, bytes: &[u8], stage: &'static str) -> Result<(), DevError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|_| DevError::Stage {
            stage,
            reason: format!("cannot create {}", parent.display()),
        })?;
    }
    let temporary = path.with_extension("tmp-dev-write");
    fs::write(&temporary, bytes).map_err(|_| DevError::Stage {
        stage,
        reason: format!("cannot write {}", path.display()),
    })?;
    fs::rename(&temporary, path).map_err(|_| DevError::Stage {
        stage,
        reason: format!("cannot activate {}", path.display()),
    })?;
    Ok(())
}

/// Builds the `installation.json` document for a dev home.
#[must_use]
pub fn installation_document(home: &Path, permanent_ae: &Path) -> serde_json::Value {
    serde_json::json!({
        "activation_state": "active",
        "finalization_state": "complete",
        "active_version": DEV_VERSION,
        "install_root": home,
        "permanent_ae_path": permanent_ae,
    })
}

/// Writes and validates the dev installation manifest.
///
/// Validation reuses the shipping loader, so a dev home the Editor would
/// refuse is reported here instead of at launch.
///
/// # Errors
///
/// Returns [`DevError::Stage`] when the manifest cannot be written or the
/// shipping loader rejects it.
pub fn provision_manifest(paths: &DevPaths) -> Result<(), DevError> {
    let document = installation_document(&paths.home, &paths.permanent_ae);
    let bytes = serde_json::to_vec_pretty(&document).map_err(|_| DevError::Stage {
        stage: "manifest",
        reason: "cannot serialize installation manifest".to_owned(),
    })?;
    write_atomic(&paths.manifest_path, &bytes, "manifest")?;
    InstallationManifest::load(&paths.manifest_path).map_err(|error| DevError::Stage {
        stage: "manifest",
        reason: format!("shipping manifest loader rejected the dev home: {error}"),
    })?;
    Ok(())
}

/// Builds the `payload-manifest.json` document for staged binaries.
///
/// # Errors
///
/// Returns [`DevError::Stage`] when a staged binary cannot be hashed.
pub fn payload_document(paths: &DevPaths) -> Result<serde_json::Value, DevError> {
    let mut files = BTreeMap::new();
    for stem in ["ae", "installer", "editor", "forge"] {
        let relative = format!("bin/{}", exe_name(stem));
        let digest = hash_file(&paths.version_root.join(&relative))?;
        files.insert(relative, digest);
    }
    Ok(serde_json::json!({
        "format_version": 1,
        "files": files,
    }))
}

/// Writes the payload manifest and gates on the shipping verifier.
///
/// # Errors
///
/// Returns [`DevError::Stage`] when the manifest cannot be written and
/// [`DevError::PayloadUnverified`] when the existing verifier reports drift.
pub fn provision_payload(paths: &DevPaths) -> Result<(), DevError> {
    let document = payload_document(paths)?;
    let bytes = serde_json::to_vec(&document).map_err(|_| DevError::Stage {
        stage: "payload",
        reason: "cannot serialize payload manifest".to_owned(),
    })?;
    write_atomic(
        &paths.version_root.join(PAYLOAD_MANIFEST_NAME),
        &bytes,
        "payload",
    )?;
    match payload::verify(&paths.version_root) {
        payload::PayloadHealth::Verified => Ok(()),
        payload::PayloadHealth::Modified(issues) => Err(DevError::PayloadUnverified {
            issues: issues.join(", "),
        }),
        payload::PayloadHealth::Unverifiable => Err(DevError::PayloadUnverified {
            issues: "payload manifest missing or unreadable after staging".to_owned(),
        }),
    }
}

/// Outcome of provisioning the native instance configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstanceOutcome {
    /// A fresh instance identity was minted for a new dev home.
    Created,
    /// The existing instance identity and dev data were preserved.
    Preserved,
}

/// Provisions Forge credentials and the native instance configuration.
///
/// Existing credentials, instance identity, and dev data are preserved: a
/// repeat invocation never re-mints them.
///
/// # Errors
///
/// Returns [`DevError::Stage`] when provisioning or validation fails.
pub fn provision_forge_home(paths: &DevPaths) -> Result<InstanceOutcome, DevError> {
    fs::create_dir_all(&paths.home).map_err(|_| DevError::Stage {
        stage: "provision",
        reason: format!("cannot create {}", paths.home.display()),
    })?;
    credentials::provision_or_load(&paths.home).map_err(|error| DevError::Stage {
        stage: "provision",
        reason: format!("cannot provision dev credentials: {error}"),
    })?;
    let credential_paths =
        ForgeCredentialPaths::from_home(&paths.home).map_err(|error| DevError::Stage {
            stage: "provision",
            reason: format!("cannot resolve dev credentials: {error}"),
        })?;
    let instance_path = NativeInstanceConfig::native_path(&paths.home);
    let (instance_id, outcome) = match fs::symlink_metadata(&instance_path) {
        Ok(_) => (
            NativeInstanceConfig::load(&instance_path)
                .map_err(|error| DevError::Stage {
                    stage: "provision",
                    reason: format!("existing dev instance is invalid: {error}"),
                })?
                .instance_id(),
            InstanceOutcome::Preserved,
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (
            artisan_editor_cli::instance::mint_instance_id().map_err(|error| DevError::Stage {
                stage: "provision",
                reason: format!("secure random source failed: {error}"),
            })?,
            InstanceOutcome::Created,
        ),
        Err(_) => {
            return Err(DevError::Stage {
                stage: "provision",
                reason: format!("cannot inspect {}", instance_path.display()),
            });
        }
    };
    let config = NativeInstanceConfig::new_with_instance_id(
        instance_id,
        paths.database_path(),
        paths.custody_path(),
        paths.readiness_path(),
        credential_paths.manifest_path().to_path_buf(),
        NativeListenerConfig::new(
            DEV_ADMISSION_TIMEOUT_MS,
            DEV_HANDSHAKE_TIMEOUT_MS,
            DEV_REQUEST_TIMEOUT_MS,
            DEV_DRAIN_TIMEOUT_MS,
            std::num::NonZeroU32::new(DEV_ADMISSION_CAPACITY).unwrap_or(std::num::NonZeroU32::MIN),
            std::num::NonZeroU32::new(DEV_REQUESTS_PER_CONNECTION)
                .unwrap_or(std::num::NonZeroU32::MIN),
        ),
        NativeRunConfig::new(NativeRunConfigInput {
            claim_lease_ms: DEV_RUN_CLAIM_LEASE_MS,
            poll_interval_ms: DEV_RUN_POLL_INTERVAL_MS,
            retry_backoff_ms: DEV_RUN_RETRY_BACKOFF_MS,
            shutdown_budget_ms: DEV_RUN_SHUTDOWN_BUDGET_MS,
            queue_capacity: DEV_RUN_QUEUE_CAPACITY,
            max_command_retries: DEV_RUN_MAX_COMMAND_RETRIES,
            prompt_delivery: DEV_RUN_PROMPT_DELIVERY.to_owned(),
            stream_after: DEV_RUN_STREAM_AFTER,
        })
        .map_err(|error| DevError::Stage {
            stage: "provision",
            reason: format!("dev instance configuration is invalid: {error}"),
        })?,
    )
    .map_err(|error| DevError::Stage {
        stage: "provision",
        reason: format!("dev instance configuration is invalid: {error}"),
    })?;
    config
        .write_to_home(&paths.home)
        .map_err(|error| DevError::Stage {
            stage: "provision",
            reason: format!("cannot write dev instance: {error}"),
        })?;
    Ok(outcome)
}

/// Refuses staging when a previous dev Forge still owns the dev home.
///
/// A stale readiness receipt is fine: the staged Editor's owned startup
/// replaces it the way production does.
///
/// # Errors
///
/// Returns [`DevError::PreviousForgeRunning`] when the readiness receipt
/// identifies a live Forge running the staged binary.
pub fn refuse_live_forge(paths: &DevPaths, forge_exe: &Path) -> Result<(), DevError> {
    match process::readiness_status(&paths.readiness_path(), forge_exe) {
        ForgeReadinessStatus::Ready(readiness) => Err(DevError::PreviousForgeRunning {
            pid: readiness.pid(),
        }),
        ForgeReadinessStatus::Missing | ForgeReadinessStatus::Invalid => Ok(()),
    }
}

/// Launches the staged Editor on the dev home and waits for it.
///
/// The child inherits the environment with `ARTISAN_HOME` pointed at the
/// dev home and the manual-forge escape hatches removed, so the Editor
/// always exercises its owned Forge custody path. Standard streams are
/// inherited so Editor output stays visible.
///
/// Returns the Editor's exit code.
///
/// # Errors
///
/// Returns [`DevError::Stage`] when the Editor cannot be spawned, and
/// [`DevError::EditorStatus`] when it exits abnormally.
pub fn launch_editor(editor_exe: &Path, home: &Path) -> Result<i32, DevError> {
    let mut command = std::process::Command::new(editor_exe);
    command
        .env(DEV_HOME_ENV, home)
        .env_remove(STRIPPED_DEV_HOME_ENV)
        .env_remove(STRIPPED_DEV_READY_ENV)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit());
    let mut child = command.spawn().map_err(|_| DevError::Stage {
        stage: "launch",
        reason: format!("cannot launch {}", editor_exe.display()),
    })?;
    let status = child.wait().map_err(|_| DevError::Stage {
        stage: "launch",
        reason: "cannot wait for the staged editor".to_owned(),
    })?;
    status.code().map_or_else(
        || {
            Err(DevError::EditorStatus {
                status: "terminated by signal".to_owned(),
            })
        },
        Ok,
    )
}

/// Formats one completed stage line (plain text, no TTY codes).
#[must_use]
pub fn stage_line(index: u32, total: u32, stage: &str, detail: &str) -> String {
    if detail.is_empty() {
        format!("dev: stage {index}/{total} {stage} ... ok")
    } else {
        format!("dev: stage {index}/{total} {stage} ... ok ({detail})")
    }
}
