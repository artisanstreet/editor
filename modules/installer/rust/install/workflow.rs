//! Activation and lifecycle operations.
//!
//! Activation takes a verified release tree through stable CLI installation,
//! the activation pointer swap, and post-activation integration. The
//! remaining public entry points maintain an existing installation: repair,
//! diagnose, uninstall, and prepare-update. Acquiring a release lives in the
//! sibling `release` module.

use std::{
    path::{Path, PathBuf},
    process::Stdio,
};

use crate::{
    background_process::{background_command, detached_background_command},
    error::{InstallerError, Result, io},
    integrations::{
        OwnedIntegration, apply_protocol, prepare_protocol, remove_protocol, verify_protocol,
    },
    manifest::{ReleaseRecord, TrustKey},
    platform::Platform,
    processes::{Retirement, RetirementPolicy, retire_superseded},
    shortcuts,
};
use chrono::Utc;
use serde::Serialize;

use super::source::ReleaseSource;
use super::{
    authority::{
        EntryKind, FileIdentity, InstallerLock, PendingMarker, PendingMarkerKind, RootMode,
        copy_owned_file, create_owned_file, ensure_owned_directory, hash_file,
        ordinary_directory_exists, ordinary_file_exists, ordinary_metadata, ordinary_path_identity,
        owned_file_identity, remove_owned_file, require_identity, sync_owned_file,
    },
    path_registry,
    state::{
        persist_protocol_record, persist_shortcut_records, read_existing_protocol,
        read_installed_state, recover_activation_pointer_swap, remove_path_in_root,
        schedule_installation_cleanup, validate_state_root,
    },
};

/// First install configures and verifies Forge, but deliberately leaves launch
/// to the editor's background handoff. That gives the window exact ownership
/// of the process it caused and lets normal window close stop that Forge. An
/// explicit autostart task remains independently owned by `ae setup --autostart`.
pub(crate) const FIRST_RUN_CONFIGURATION_COMMANDS: [&[&str]; 3] =
    [&["setup"], &["doctor"], &["status"]];

pub struct InstallIntegrationOptions {
    /// Whether this install may own the `artisan://` handler. A secondary
    /// install beside an existing installation must leave the handler with
    /// its current owner rather than fail on finding it taken.
    pub register_protocol: bool,
    /// Whether this install owns the desktop and Start Menu launchers.
    pub register_shortcuts: bool,
    /// Whether this install puts its permanent `ae` on the user PATH. A
    /// side-by-side install (such as the `dev` channel) must not shadow the
    /// primary installation's `ae`.
    pub register_path: bool,
}

pub struct InstallOptions {
    /// Where the release comes from.
    pub source: ReleaseSource,
    pub platform: Platform,
    pub install_root: PathBuf,
    pub trust: TrustKey,
    /// Channel the release must belong to; `None` accepts the release's own
    /// channel (an existing installation still pins its channel).
    pub expected_channel: Option<String>,
    pub run_setup: bool,
    /// Whether a Forge stopped by retirement is started again from the new
    /// release. A caller that relaunches the Editor itself leaves this off so
    /// the Editor owns the Forge it starts.
    pub restore_forge: bool,
    pub integrations: InstallIntegrationOptions,
    /// `None` leaves superseded editor and Forge processes running. A policy
    /// retires them and controls whether a stuck Forge may be ended.
    pub retirement: Option<RetirementPolicy>,
}

/// Performs the shared activation steps for a release tree that has already
/// been verified: stable CLI installation, protocol and shortcut preparation,
/// the activation pointer swap, and integration application. Returns the
/// retirement result so the caller can restore Forge after the final step.
pub(super) fn activate_release(
    root_lock: &InstallerLock,
    options: &InstallOptions,
    record: &ReleaseRecord,
    release: &Path,
) -> Result<Retirement> {
    let lifecycle_ae = release_cli(release)?;
    let existing_protocol = read_existing_protocol(&options.install_root)?;
    let bootstrap = versioned_installer_path(release);
    if !ordinary_file_exists(&bootstrap)? {
        return Err(InstallerError::MissingInstaller(bootstrap));
    }
    root_lock.fence()?;
    let retirement = retire_for(options, release, &lifecycle_ae)?;
    let stable_ae = install_stable_cli(
        root_lock,
        &options.install_root,
        release,
        options.integrations.register_path,
    )?;
    let protocol = if options.integrations.register_protocol {
        prepare_protocol(&options.platform, &stable_ae, existing_protocol.as_ref())?
    } else {
        None
    };
    let launchers = planned_shortcuts(options, &stable_ae, release);
    root_lock.fence()?;
    let activation_launchers = shortcut_records(&launchers)?;
    activate(
        root_lock,
        &options.install_root,
        release,
        record,
        options,
        &ActivationIntegrations {
            stable_ae: &stable_ae,
            protocol: protocol.as_ref(),
            launchers: &activation_launchers,
        },
    )?;
    root_lock.fence()?;
    if options.integrations.register_protocol {
        apply_protocol(&options.platform, &stable_ae, existing_protocol.as_ref())?;
    }
    root_lock.fence()?;
    shortcuts::apply(&launchers)?;
    Ok(retirement)
}

#[derive(Debug, Serialize)]
pub(crate) struct Components {
    editor: bool,
    forge: bool,
}

pub(crate) fn installed_components() -> Components {
    Components {
        editor: true,
        forge: true,
    }
}

/// The launchers this run owns, or none when the caller opted out.
fn planned_shortcuts(
    options: &InstallOptions,
    stable_ae: &Path,
    release: &Path,
) -> Vec<shortcuts::ShortcutTarget> {
    if options.integrations.register_shortcuts {
        shortcuts::targets(&options.platform, stable_ae, release)
    } else {
        Vec::new()
    }
}

fn shortcut_records(targets: &[shortcuts::ShortcutTarget]) -> Result<Vec<OwnedIntegration>> {
    targets
        .iter()
        .map(shortcuts::ShortcutTarget::owned)
        .collect()
}

/// Closes the old editor and proves Forge can stop before activation changes
/// any durable pointer or integration. A busy Forge therefore cancels the
/// update while the prior installation remains authoritative.
fn retire_for(options: &InstallOptions, release: &Path, stable_ae: &Path) -> Result<Retirement> {
    let Some(policy) = options.retirement else {
        return Ok(Retirement::default());
    };

    let retirement = retire_superseded(&options.install_root, release, stable_ae, policy)?;
    if !retirement.is_empty() {
        println!(
            "retired superseded instances: {} editor, {} forge",
            retirement.editors_closed, retirement.forges_stopped
        );
    }
    Ok(retirement)
}

/// A maintenance update has no editor launch after the installer returns, so
/// preserve a Forge that was running before the update by starting the newly
/// activated version. Setup-driven installs deliberately skip this: their
/// caller opens the editor, whose background handoff must own the Forge it
/// starts so window close can stop that exact process.
pub(super) fn restore_retired_forge(
    options: &InstallOptions,
    release: &Path,
    retirement: Retirement,
) -> Result<()> {
    if options.restore_forge && should_restore_retired_forge(options.run_setup, retirement) {
        invoke_ae(release, &["start"])?;
    }
    Ok(())
}

pub(super) fn run_setup_sequence(release: &Path) -> Result<()> {
    for arguments in FIRST_RUN_CONFIGURATION_COMMANDS {
        invoke_ae(release, arguments)?;
    }
    Ok(())
}

pub(crate) fn should_restore_retired_forge(run_setup: bool, retirement: Retirement) -> bool {
    !run_setup && retirement.forges_stopped > 0
}

struct ActivationIntegrations<'a> {
    stable_ae: &'a Path,
    protocol: Option<&'a OwnedIntegration>,
    launchers: &'a [OwnedIntegration],
}

fn activate(
    lock: &InstallerLock,
    root: &Path,
    release: &Path,
    record: &ReleaseRecord,
    options: &InstallOptions,
    integrations: &ActivationIntegrations<'_>,
) -> Result<()> {
    lock.fence()?;
    let next = root.join(".installation.json.tmp");
    let current = root.join("installation.json");
    let now = Utc::now().to_rfc3339();
    let mut integration_records = serde_json::Map::from_iter([(
        "ae_path".to_owned(),
        serde_json::to_value(OwnedIntegration {
            path: integrations.stable_ae.display().to_string(),
            fingerprint: hash_file(&release.join("bin").join(if cfg!(windows) {
                "ae.exe"
            } else {
                "ae"
            }))?,
        })
        .map_err(InstallerError::InvalidPayload)?,
    )]);
    if let Some(protocol) = integrations.protocol {
        integration_records.insert(
            "protocol".to_owned(),
            serde_json::to_value(protocol).map_err(InstallerError::InvalidPayload)?,
        );
    }
    if !integrations.launchers.is_empty() {
        integration_records.insert(
            "shortcuts".to_owned(),
            serde_json::to_value(integrations.launchers).map_err(InstallerError::InvalidPayload)?,
        );
    }
    let contents = serde_json::json!({
        "format_version": 1,
        "install_root": root,
        "platform": options.platform.os,
        "architecture": options.platform.arch,
        "channel": record.channel.as_str(),
        "components": installed_components(),
        "integrations": integration_records,
        "installed_at": now,
        "updated_at": now,
        "activation_state": "active",
        "finalization_state": "complete",
        "active_version": record.product_version.as_str(),
        "permanent_ae_path": integrations.stable_ae,
        "artifact": {
            "artifact_id": record.artifact_id.as_str(),
            "sha256": record.artifact_sha256.as_str(),
            "signing_key_id": record.signing_key_id.as_str(),
        },
        "transaction": { "state": "idle" }
    });
    let mut file = create_owned_file(&next)?;
    serde_json::to_writer(&mut file, &contents)
        .map_err(|error| InstallerError::Archive(error.to_string()))?;
    let next_identity = sync_owned_file(&next, &file)?;
    drop(file);
    require_identity(&next, EntryKind::File, next_identity)?;
    let previous = root.join(".installation.json.previous");
    lock.fence()?;
    remove_owned_file(&previous)?;
    if let Some(current_identity) = owned_file_identity(&current)? {
        require_identity(&current, EntryKind::File, current_identity)?;
        std::fs::rename(&current, &previous).map_err(io(&current))?;
    }
    match std::fs::symlink_metadata(&current) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Ok(_) | Err(_) => return Err(InstallerError::UnsafeOwnedPath),
    }
    if let Err(source) = std::fs::rename(&next, &current) {
        if let Some(previous_identity) = owned_file_identity(&previous)? {
            require_identity(&previous, EntryKind::File, previous_identity)?;
            let _ = std::fs::rename(&previous, &current);
        }
        return Err(io(&current)(source));
    }
    remove_owned_file(&previous)?;
    Ok(())
}

fn install_stable_cli(
    lock: &InstallerLock,
    root: &Path,
    release: &Path,
    integrate_path: bool,
) -> Result<PathBuf> {
    lock.fence()?;
    let source = release_cli(release)?;
    let bin = root.join("bin");
    ensure_owned_directory(&bin)?;
    let stable = bin.join(if cfg!(windows) { "ae.exe" } else { "ae" });
    let temporary = bin.join(".ae.next");
    let temporary_identity = copy_owned_file(&source, &temporary)?;
    match std::fs::symlink_metadata(&stable) {
        Ok(metadata) => {
            if !ordinary_metadata(&metadata, EntryKind::File) {
                return Err(InstallerError::UnsafeOwnedPath);
            }
            let stable_identity = ordinary_path_identity(&stable, EntryKind::File)
                .map_err(|()| InstallerError::UnsafeOwnedPath)?;
            lock.fence()?;
            require_identity(&stable, EntryKind::File, stable_identity)?;
            if hash_file(&stable)? == hash_file(&source)? {
                require_identity(&stable, EntryKind::File, stable_identity)?;
                require_identity(&temporary, EntryKind::File, temporary_identity)?;
                remove_owned_file(&temporary)?;
                lock.fence()?;
                if integrate_path {
                    path_registry::integrate_path(&bin)?;
                }
                return Ok(stable);
            }
            require_identity(&stable, EntryKind::File, stable_identity)?;
            if let Err(remove_error) = std::fs::remove_file(&stable) {
                #[cfg(windows)]
                {
                    let _ = remove_error;
                    schedule_stable_cli_replacement(
                        lock,
                        &temporary,
                        &stable,
                        temporary_identity,
                        stable_identity,
                    )?;
                    lock.fence()?;
                    if integrate_path {
                        path_registry::integrate_path(&bin)?;
                    }
                    return Ok(stable);
                }
                #[cfg(not(windows))]
                return Err(io(&stable)(remove_error));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(InstallerError::UnsafeOwnedPath),
    }
    lock.fence()?;
    require_identity(&temporary, EntryKind::File, temporary_identity)?;
    match std::fs::symlink_metadata(&stable) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Ok(_) | Err(_) => return Err(InstallerError::UnsafeOwnedPath),
    }
    std::fs::rename(&temporary, &stable).map_err(io(&stable))?;
    lock.fence()?;
    if integrate_path {
        path_registry::integrate_path(&bin)?;
    }
    Ok(stable)
}

fn release_cli(release: &Path) -> Result<PathBuf> {
    let bin = release.join("bin");
    let executable = release
        .join("bin")
        .join(if cfg!(windows) { "ae.exe" } else { "ae" });
    if !ordinary_directory_exists(&bin)? || !ordinary_file_exists(&executable)? {
        return Err(InstallerError::MissingCli(executable));
    }
    Ok(executable)
}

pub(crate) fn versioned_installer_path(release: &Path) -> PathBuf {
    release.join("bin").join(if cfg!(windows) {
        "installer.exe"
    } else {
        "installer"
    })
}

#[cfg(windows)]
fn schedule_stable_cli_replacement(
    lock: &InstallerLock,
    source: &Path,
    destination: &Path,
    source_identity: FileIdentity,
    destination_identity: FileIdentity,
) -> Result<()> {
    lock.fence()?;
    let marker = PendingMarker::create(lock, PendingMarkerKind::AeReplacement)?;
    lock.fence()?;
    require_identity(source, EntryKind::File, source_identity)?;
    match std::fs::symlink_metadata(destination) {
        Ok(metadata) if ordinary_metadata(&metadata, EntryKind::File) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Ok(_) | Err(_) => return Err(InstallerError::UnsafeOwnedPath),
    }
    require_identity(destination, EntryKind::File, destination_identity)?;
    lock.fence()?;
    require_identity(source, EntryKind::File, source_identity)?;
    require_identity(destination, EntryKind::File, destination_identity)?;
    detached_background_command("cmd.exe")
        .args([
            "/d",
            "/s",
            "/c",
            "ping 127.0.0.1 -n 3 > nul & move /y \"%ARTISAN_AE_SOURCE%\" \"%ARTISAN_AE_DESTINATION%\" > nul && rmdir \"%ARTISAN_AE_MARKER%\" > nul",
        ])
        .env("ARTISAN_AE_SOURCE", source)
        .env("ARTISAN_AE_DESTINATION", destination)
        .env("ARTISAN_AE_MARKER", &marker.path)
        .spawn()
        .map_err(|_| InstallerError::LifecycleHelper)?;
    Ok(())
}

/// Restores the stable launcher, protocol handler, and shortcuts of the
/// installation at `root`, then runs `ae doctor`.
///
/// # Errors
///
/// Returns [`InstallerError`] when the installation state is invalid, the
/// active release is missing, or an integration cannot be restored.
pub fn repair(root: &Path) -> Result<()> {
    let root_lock = InstallerLock::acquire(root, RootMode::Existing)?;
    root_lock.fence()?;
    recover_activation_pointer_swap(&root_lock)?;
    let state = read_installed_state(root)?;
    validate_state_root(root, &state)?;
    let release = root.join("versions").join(&state.active_version);
    if !ordinary_directory_exists(&root.join("versions"))? {
        return Err(InstallerError::InvalidInstallation(
            "the installation versions directory is missing or unsafe".to_owned(),
        ));
    }
    let bootstrap = versioned_installer_path(&release);
    if !ordinary_directory_exists(&release)? {
        return Err(InstallerError::MissingInstaller(bootstrap));
    }
    if !ordinary_file_exists(&bootstrap)? {
        return Err(InstallerError::MissingInstaller(bootstrap));
    }
    let stable = install_stable_cli(&root_lock, root, &release, !is_local_channel_root(root))?;
    if stable != state.permanent_ae_path {
        return Err(InstallerError::InvalidInstallation(
            "permanent ae path is outside the bootstrap-owned layout".to_owned(),
        ));
    }
    let platform = Platform::detect()?;
    let existing_protocol = state.integrations.protocol.as_ref();
    // An installation with no recorded protocol ownership was installed with
    // `--skip-protocol` beside a primary installation. Repairing it must not
    // adopt the handler the primary owns ÔÇö finding it registered elsewhere is
    // this installation's healthy state, not damage to fix.
    if existing_protocol.is_some() {
        root_lock.fence()?;
        let protocol = prepare_protocol(&platform, &stable, existing_protocol)?;
        if protocol.as_ref() != state.integrations.protocol.as_ref()
            && let Some(protocol) = protocol.as_ref()
        {
            root_lock.fence()?;
            persist_protocol_record(root, protocol)?;
        }
        root_lock.fence()?;
        apply_protocol(&platform, &stable, existing_protocol)?;
    }
    // Launchers are rewritten rather than merely checked: their icon is taken
    // from the versioned editor executable, so every update leaves the
    // previous release's path behind in an otherwise healthy shortcut.
    let launchers = shortcuts::targets(&platform, &stable, &release);
    if !state.integrations.shortcuts.is_empty() || !launchers.is_empty() {
        root_lock.fence()?;
        let records = shortcut_records(&launchers)?;
        root_lock.fence()?;
        shortcuts::apply(&launchers)?;
        root_lock.fence()?;
        persist_shortcut_records(root, &records)?;
    }
    root_lock.fence()?;
    invoke_ae_diagnostic(&release, &["doctor"])
}

/// Verifies the stable launcher and protocol handler of the installation at
/// `root` without changing them.
///
/// # Errors
///
/// Returns [`InstallerError`] when the installation state or an integration
/// is invalid.
pub fn diagnose(root: &Path) -> Result<()> {
    let root_lock = InstallerLock::acquire(root, RootMode::Existing)?;
    root_lock.fence()?;
    recover_activation_pointer_swap(&root_lock)?;
    let state = read_installed_state(root)?;
    validate_state_root(root, &state)?;
    let stable = root
        .join("bin")
        .join(if cfg!(windows) { "ae.exe" } else { "ae" });
    if !ordinary_directory_exists(&root.join("bin"))?
        || stable != state.permanent_ae_path
        || !ordinary_file_exists(&stable)?
    {
        return Err(InstallerError::InvalidInstallation(
            "permanent ae path is missing or outside the bootstrap-owned layout".to_owned(),
        ));
    }
    root_lock.fence()?;
    verify_protocol(
        &Platform::detect()?,
        &stable,
        state.integrations.protocol.as_ref(),
    )
}

fn invoke_ae_diagnostic(release: &Path, arguments: &[&str]) -> Result<()> {
    let executable = release
        .join("bin")
        .join(if cfg!(windows) { "ae.exe" } else { "ae" });
    if !ordinary_file_exists(&executable)? {
        return Err(InstallerError::MissingCli(executable));
    }
    // Doctor reports Forge-instance problems independently. Repair owns the
    // installation invariants above and must not recurse through `--fix`.
    let _status = background_command(&executable)
        .args(arguments)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(io(&executable))?;
    Ok(())
}

/// Removes the installation at `root` and its owned integrations, and its
/// Forge data only when `remove_data` is set.
///
/// # Errors
///
/// Returns [`InstallerError`] when the installation state is invalid or an
/// owned path cannot be removed safely.
pub fn uninstall(root: &Path, remove_data: bool) -> Result<()> {
    let root_lock = InstallerLock::acquire(root, RootMode::Existing)?;
    root_lock.fence()?;
    recover_activation_pointer_swap(&root_lock)?;
    let state = read_installed_state(root)?;
    validate_state_root(root, &state)?;
    let stable = root
        .join("bin")
        .join(if cfg!(windows) { "ae.exe" } else { "ae" });
    root_lock.fence()?;
    remove_protocol(
        &Platform::detect()?,
        &state.permanent_ae_path,
        state.integrations.protocol.as_ref(),
    )?;
    root_lock.fence()?;
    shortcuts::remove(
        &shortcuts::targets(
            &Platform::detect()?,
            &state.permanent_ae_path,
            &root.join("versions").join(&state.active_version),
        ),
        &state.integrations.shortcuts,
    )?;
    if let Some(integration) = state.integrations.ae_path {
        let path = Path::new(&integration.path);
        if path == stable
            && let Ok(metadata) = std::fs::symlink_metadata(path)
            && ordinary_metadata(&metadata, EntryKind::File)
            && hash_file(path)? == integration.fingerprint
        {
            root_lock.fence()?;
            std::fs::remove_file(path).map_err(io(path))?;
        }
    }
    root_lock.fence()?;
    path_registry::remove_path_integration(&root.join("bin"))?;
    root_lock.fence()?;
    remove_path_in_root(root, &root.join("bin"))?;
    root_lock.fence()?;
    remove_path_in_root(root, &root.join("installation.json"))?;
    if remove_data {
        // The home hosts one Forge instance at its root; legacy `profiles/`
        // trees predate the single-instance layout and are removed alongside.
        for name in [
            "config.json",
            "secrets.json",
            "state.json",
            "forge.log",
            "data",
            "profiles",
        ] {
            root_lock.fence()?;
            remove_path_in_root(root, &root.join(name))?;
        }
    }
    root_lock.fence()?;
    schedule_installation_cleanup(&root_lock, root)
}

/// Closes the Editor and retires Forge of the installation at `root` ahead
/// of a locally built release.
///
/// # Errors
///
/// Returns [`InstallerError`] when the root is busy or invalid, or a running
/// instance cannot be retired under `retirement`.
pub fn prepare_update(root: &Path, retirement: Option<RetirementPolicy>) -> Result<()> {
    let root_lock = InstallerLock::acquire(root, RootMode::Existing)?;
    root_lock.fence()?;
    recover_activation_pointer_swap(&root_lock)?;
    let Some(retirement_policy) = retirement else {
        return Ok(());
    };
    let lifecycle_ae = root
        .join("bin")
        .join(if cfg!(windows) { "ae.exe" } else { "ae" });
    if !ordinary_directory_exists(&root.join("bin"))? {
        return Err(InstallerError::MissingCli(lifecycle_ae));
    }
    if !ordinary_file_exists(&lifecycle_ae)? {
        return Err(InstallerError::MissingCli(lifecycle_ae));
    }
    let incoming_release = root.join(".incoming-release");
    if !ordinary_directory_exists(&root.join("versions"))? {
        return Err(InstallerError::InvalidInstallation(
            "the installation versions directory is missing or unsafe".to_owned(),
        ));
    }
    root_lock.fence()?;
    let retirement = retire_superseded(root, &incoming_release, &lifecycle_ae, retirement_policy)?;
    if !retirement.is_empty() {
        println!(
            "prepared update: closed {} editor, stopped {} forge",
            retirement.editors_closed, retirement.forges_stopped
        );
    }
    Ok(())
}

pub(super) fn invoke_ae(release: &Path, arguments: &[&str]) -> Result<()> {
    let executable = release
        .join("bin")
        .join(if cfg!(windows) { "ae.exe" } else { "ae" });
    if !ordinary_file_exists(&executable)? {
        return Err(InstallerError::MissingCli(executable));
    }
    let status = background_command(&executable)
        .args(arguments)
        .status()
        .map_err(io(&executable))?;
    if !status.success() {
        return Err(InstallerError::CliFailed {
            command: arguments.join(" "),
            status: status.to_string(),
        });
    }
    Ok(())
}

/// Whether `root` holds a side-by-side local (`dev`) installation, whose
/// permanent `ae` must never be put on the user PATH.
fn is_local_channel_root(root: &Path) -> bool {
    std::fs::read(root.join("installation.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|document| {
            document
                .get("channel")
                .and_then(serde_json::Value::as_str)
                .map(|channel| channel == crate::local::LOCAL_CHANNEL)
        })
        .unwrap_or(false)
}
