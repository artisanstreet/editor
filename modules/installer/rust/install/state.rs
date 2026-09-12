//! Durable installation state and integration registry.
//!
//! Reads and validates `installation.json`, persists the protocol and shortcut
//! records the installer owns, recovers the activation-pointer swap after a
//! crash, and owns the validated removal and delayed cleanup of an
//! installation root. Nothing here fetches or extracts release artifacts.

use std::{
    io::Read,
    path::{Component, Path, PathBuf},
};

use serde::Deserialize;

use crate::{
    background_process::detached_background_command,
    error::{InstallerError, Result, io},
    integrations::OwnedIntegration,
};

use super::authority::{
    EntryKind, FileIdentity, INSTALLER_LOCK_NAME, InstallerLock, PendingMarker, PendingMarkerKind,
    create_owned_file, identity_from_file, open_for_read, ordinary_file_exists, ordinary_metadata,
    ordinary_path_identity, owned_file_identity, remove_owned_file, require_identity,
    sync_owned_file,
};

#[derive(Debug, Deserialize)]
pub(crate) struct InstalledState {
    pub(crate) activation_state: String,
    pub(crate) active_version: String,
    pub(crate) install_root: PathBuf,
    pub(crate) permanent_ae_path: PathBuf,
    #[serde(default)]
    pub(crate) integrations: InstalledIntegrations,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct InstalledIntegrations {
    pub(crate) ae_path: Option<OwnedIntegration>,
    pub(crate) protocol: Option<OwnedIntegration>,
    /// Absent in installations predating installer-owned launchers, which is
    /// why removal and repair both treat an empty record as "not ours".
    #[serde(default)]
    pub(crate) shortcuts: Vec<OwnedIntegration>,
}

#[derive(Debug, Deserialize)]
struct ActivationPointerState {
    activation_state: String,
    finalization_state: String,
    active_version: String,
    install_root: PathBuf,
    permanent_ae_path: PathBuf,
}

pub(crate) struct ValidatedActivationPointer {
    path: PathBuf,
    identity: FileIdentity,
}

pub(crate) fn activation_pointer_paths(root: &Path) -> [PathBuf; 3] {
    [
        root.join("installation.json"),
        root.join(".installation.json.tmp"),
        root.join(".installation.json.previous"),
    ]
}

fn ambiguous_activation_error() -> InstallerError {
    InstallerError::InstallationActivationTransactionAmbiguous
}

/// Recover only the three files that participate in the activation pointer
/// swap. Every file is inspected before any residue is removed or restored so
/// an unsafe transaction remains available for a later, informed retry.
pub(crate) fn recover_activation_pointer_swap(lock: &InstallerLock) -> Result<()> {
    lock.fence().map_err(|_| ambiguous_activation_error())?;
    let [current_path, temporary_path, previous_path] = activation_pointer_paths(&lock.root);
    let current = inspect_activation_pointer(&lock.root, &current_path)?;
    let temporary = inspect_activation_pointer(&lock.root, &temporary_path)?;
    let previous = inspect_activation_pointer(&lock.root, &previous_path)?;

    lock.fence().map_err(|_| ambiguous_activation_error())?;
    if let Some(current) = current.as_ref() {
        if let Some(temporary) = temporary.as_ref() {
            revalidate_activation_pointer(Some(current), &current_path)?;
            revalidate_activation_pointer(Some(temporary), &temporary_path)?;
            revalidate_activation_pointer(previous.as_ref(), &previous_path)?;
            remove_validated_activation_pointer(temporary)?;
        }
        if let Some(previous) = previous.as_ref() {
            revalidate_activation_pointer(Some(current), &current_path)?;
            revalidate_activation_pointer(Some(previous), &previous_path)?;
            remove_validated_activation_pointer(previous)?;
        }
        return Ok(());
    }

    if let Some(previous) = previous.as_ref() {
        revalidate_activation_pointer(None, &current_path)?;
        revalidate_activation_pointer(Some(previous), &previous_path)?;
        revalidate_activation_pointer(temporary.as_ref(), &temporary_path)?;
        std::fs::rename(&previous.path, &current_path).map_err(|_| ambiguous_activation_error())?;
        require_identity(&current_path, EntryKind::File, previous.identity)
            .map_err(|_| ambiguous_activation_error())?;
        if let Some(temporary) = temporary.as_ref() {
            revalidate_activation_pointer(Some(temporary), &temporary_path)?;
            require_identity(&current_path, EntryKind::File, previous.identity)
                .map_err(|_| ambiguous_activation_error())?;
            remove_validated_activation_pointer(temporary)?;
        }
        return Ok(());
    }

    if let Some(temporary) = temporary.as_ref() {
        revalidate_activation_pointer(None, &current_path)?;
        revalidate_activation_pointer(Some(temporary), &temporary_path)?;
        revalidate_activation_pointer(None, &previous_path)?;
        remove_validated_activation_pointer(temporary)?;
    }
    Ok(())
}

pub(crate) fn inspect_activation_pointer(
    root: &Path,
    path: &Path,
) -> Result<Option<ValidatedActivationPointer>> {
    let Some(identity) = owned_file_identity(path).map_err(|_| ambiguous_activation_error())?
    else {
        return Ok(None);
    };
    let mut file =
        open_for_read(path, EntryKind::File).map_err(|_| ambiguous_activation_error())?;
    if identity_from_file(&file, EntryKind::File).map_err(|()| ambiguous_activation_error())?
        != identity
    {
        return Err(ambiguous_activation_error());
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|_| ambiguous_activation_error())?;
    if identity_from_file(&file, EntryKind::File).map_err(|()| ambiguous_activation_error())?
        != identity
    {
        return Err(ambiguous_activation_error());
    }
    require_identity(path, EntryKind::File, identity).map_err(|_| ambiguous_activation_error())?;
    let state: ActivationPointerState =
        serde_json::from_slice(&bytes).map_err(|_| ambiguous_activation_error())?;
    validate_activation_pointer_state(root, &state)?;
    require_identity(path, EntryKind::File, identity).map_err(|_| ambiguous_activation_error())?;
    Ok(Some(ValidatedActivationPointer {
        path: path.to_path_buf(),
        identity,
    }))
}

fn validate_activation_pointer_state(root: &Path, state: &ActivationPointerState) -> Result<()> {
    let stable = root
        .join("bin")
        .join(if cfg!(windows) { "ae.exe" } else { "ae" });
    if state.activation_state != "active"
        || state.finalization_state != "complete"
        || state.install_root.as_path() != root
        || state.permanent_ae_path.as_path() != stable.as_path()
        || !is_safe_release_component(&state.active_version)
    {
        return Err(ambiguous_activation_error());
    }
    Ok(())
}

fn revalidate_activation_pointer(
    pointer: Option<&ValidatedActivationPointer>,
    path: &Path,
) -> Result<()> {
    match pointer {
        Some(pointer) => {
            let identity = owned_file_identity(path).map_err(|_| ambiguous_activation_error())?;
            if identity != Some(pointer.identity) {
                return Err(ambiguous_activation_error());
            }
        }
        None => match std::fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) | Err(_) => return Err(ambiguous_activation_error()),
        },
    }
    Ok(())
}

pub(crate) fn remove_validated_activation_pointer(
    pointer: &ValidatedActivationPointer,
) -> Result<()> {
    let identity = owned_file_identity(&pointer.path).map_err(|_| ambiguous_activation_error())?;
    if identity != Some(pointer.identity) {
        return Err(ambiguous_activation_error());
    }
    remove_owned_file(&pointer.path).map_err(|_| ambiguous_activation_error())
}

pub(crate) fn read_installed_state(root: &Path) -> Result<InstalledState> {
    let path = root.join("installation.json");
    ordinary_file_exists(&path)?;
    let bytes = std::fs::read(&path).map_err(io(&path))?;
    serde_json::from_slice(&bytes).map_err(InstallerError::InvalidPayload)
}

pub(crate) fn read_existing_protocol(root: &Path) -> Result<Option<OwnedIntegration>> {
    let path = root.join("installation.json");
    if !ordinary_file_exists(&path)? {
        return Ok(None);
    }
    read_installed_state(root).map(|state| state.integrations.protocol)
}

pub(crate) fn persist_protocol_record(root: &Path, protocol: &OwnedIntegration) -> Result<()> {
    let current = root.join("installation.json");
    let next = root.join(".installation.json.protocol");
    let bytes = std::fs::read(&current).map_err(io(&current))?;
    let mut document: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(InstallerError::InvalidPayload)?;
    let integrations = document
        .get_mut("integrations")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or_else(|| {
            InstallerError::InvalidInstallation(
                "installation manifest integrations are missing".to_owned(),
            )
        })?;
    integrations.insert(
        "protocol".to_owned(),
        serde_json::to_value(protocol).map_err(InstallerError::InvalidPayload)?,
    );
    let mut file = create_owned_file(&next)?;
    serde_json::to_writer(&mut file, &document).map_err(InstallerError::InvalidPayload)?;
    let next_identity = sync_owned_file(&next, &file)?;
    drop(file);
    require_identity(&next, EntryKind::File, next_identity)?;

    let previous = root.join(".installation.json.protocol.previous");
    remove_owned_file(&previous)?;
    let current_identity = owned_file_identity(&current)?.ok_or(InstallerError::UnsafeOwnedPath)?;
    require_identity(&current, EntryKind::File, current_identity)?;
    std::fs::rename(&current, &previous).map_err(io(&current))?;
    if let Err(source) = std::fs::rename(&next, &current) {
        if let Some(previous_identity) = owned_file_identity(&previous)? {
            require_identity(&previous, EntryKind::File, previous_identity)?;
            let _ = std::fs::rename(&previous, &current);
        }
        return Err(io(&current)(source));
    }
    remove_owned_file(&previous)
}

/// Records the launchers a repair just rewrote, through the same
/// write-and-swap the protocol record uses so an interrupted repair never
/// leaves the manifest half-written.
pub(crate) fn persist_shortcut_records(root: &Path, launchers: &[OwnedIntegration]) -> Result<()> {
    let current = root.join("installation.json");
    let next = root.join(".installation.json.shortcuts");
    let bytes = std::fs::read(&current).map_err(io(&current))?;
    let mut document: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(InstallerError::InvalidPayload)?;
    let integrations = document
        .get_mut("integrations")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or_else(|| {
            InstallerError::InvalidInstallation(
                "installation manifest integrations are missing".to_owned(),
            )
        })?;
    integrations.insert(
        "shortcuts".to_owned(),
        serde_json::to_value(launchers).map_err(InstallerError::InvalidPayload)?,
    );
    let mut file = create_owned_file(&next)?;
    serde_json::to_writer(&mut file, &document).map_err(InstallerError::InvalidPayload)?;
    let next_identity = sync_owned_file(&next, &file)?;
    drop(file);
    require_identity(&next, EntryKind::File, next_identity)?;

    let previous = root.join(".installation.json.shortcuts.previous");
    remove_owned_file(&previous)?;
    let current_identity = owned_file_identity(&current)?.ok_or(InstallerError::UnsafeOwnedPath)?;
    require_identity(&current, EntryKind::File, current_identity)?;
    std::fs::rename(&current, &previous).map_err(io(&current))?;
    if let Err(source) = std::fs::rename(&next, &current) {
        if let Some(previous_identity) = owned_file_identity(&previous)? {
            require_identity(&previous, EntryKind::File, previous_identity)?;
            let _ = std::fs::rename(&previous, &current);
        }
        return Err(io(&current)(source));
    }
    remove_owned_file(&previous)
}

pub(crate) fn validate_state_root(root: &Path, state: &InstalledState) -> Result<()> {
    let stable = root
        .join("bin")
        .join(if cfg!(windows) { "ae.exe" } else { "ae" });
    if state.activation_state != "active"
        || state.install_root != root
        || state.permanent_ae_path != stable
        || !is_safe_release_component(&state.active_version)
    {
        return Err(InstallerError::InvalidInstallation(
            "installation manifest does not own the requested root".to_owned(),
        ));
    }
    Ok(())
}

fn is_safe_release_component(value: &str) -> bool {
    if value.is_empty() || value.contains('\0') || value.contains(['/', '\\']) {
        return false;
    }
    let mut components = Path::new(value).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

pub(crate) fn remove_path_in_root(root: &Path, path: &Path) -> Result<()> {
    if !path.starts_with(root)
        || path == root
        || path.parent() != Some(root)
        || path.file_name() == Some(std::ffi::OsStr::new(INSTALLER_LOCK_NAME))
    {
        return Err(InstallerError::InvalidInstallation(
            "refusing removal outside the installation root".to_owned(),
        ));
    }
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(InstallerError::UnsafeOwnedPath),
    };
    if !ordinary_metadata(
        &metadata,
        if metadata.is_dir() {
            EntryKind::Directory
        } else {
            EntryKind::File
        },
    ) {
        return Err(InstallerError::UnsafeOwnedPath);
    }
    if metadata.is_dir() {
        let expected = ordinary_path_identity(path, EntryKind::Directory)
            .map_err(|()| InstallerError::UnsafeOwnedPath)?;
        validate_owned_tree(path)?;
        require_identity(path, EntryKind::Directory, expected)?;
        match std::fs::remove_dir_all(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(InstallerError::UnsafeOwnedPath),
        }
    } else {
        let expected = ordinary_path_identity(path, EntryKind::File)
            .map_err(|()| InstallerError::UnsafeOwnedPath)?;
        require_identity(path, EntryKind::File, expected)?;
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(InstallerError::UnsafeOwnedPath),
        }
    }
    Ok(())
}

fn validate_owned_tree(path: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| InstallerError::UnsafeOwnedPath)?;
    if !ordinary_metadata(&metadata, EntryKind::Directory) {
        return Err(InstallerError::UnsafeOwnedPath);
    }
    let identity = ordinary_path_identity(path, EntryKind::Directory)
        .map_err(|()| InstallerError::UnsafeOwnedPath)?;
    for entry in std::fs::read_dir(path).map_err(|_| InstallerError::UnsafeOwnedPath)? {
        let entry = entry.map_err(|_| InstallerError::UnsafeOwnedPath)?;
        let child = entry.path();
        let metadata =
            std::fs::symlink_metadata(&child).map_err(|_| InstallerError::UnsafeOwnedPath)?;
        if ordinary_metadata(&metadata, EntryKind::Directory) {
            validate_owned_tree(&child)?;
        } else if !ordinary_metadata(&metadata, EntryKind::File) {
            return Err(InstallerError::UnsafeOwnedPath);
        } else {
            ordinary_path_identity(&child, EntryKind::File)
                .map_err(|()| InstallerError::UnsafeOwnedPath)?;
        }
    }
    require_identity(path, EntryKind::Directory, identity)?;
    Ok(())
}

pub(crate) fn schedule_installation_cleanup(lock: &InstallerLock, root: &Path) -> Result<()> {
    let versions = root.join("versions");
    match std::fs::symlink_metadata(&versions) {
        Ok(_) => validate_owned_tree(&versions)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(InstallerError::UnsafeOwnedPath),
    }
    lock.fence()?;
    let marker = PendingMarker::create(lock, PendingMarkerKind::Cleanup)?;
    match std::fs::symlink_metadata(&versions) {
        Ok(_) => validate_owned_tree(&versions)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(InstallerError::UnsafeOwnedPath),
    }
    lock.fence()?;
    #[cfg(windows)]
    {
        let mut command = detached_background_command("cmd.exe");
        let script = "ping 127.0.0.1 -n 3 > nul & if exist \"%ARTISAN_VERSIONS%\" (rmdir /s /q \"%ARTISAN_VERSIONS%\" > nul) & if not exist \"%ARTISAN_VERSIONS%\" (rmdir \"%ARTISAN_CLEANUP_MARKER%\" > nul)";
        command.args(["/d", "/s", "/c", script]);
        command.env("ARTISAN_VERSIONS", versions);
        command.env("ARTISAN_CLEANUP_MARKER", &marker.path);
        command
            .spawn()
            .map_err(|_| InstallerError::LifecycleHelper)?;
    }
    #[cfg(unix)]
    {
        std::process::Command::new("sh")
            .args([
                "-c",
                "sleep 1; if [ -e \"$1\" ] || [ -L \"$1\" ]; then [ -d \"$1\" ] && [ ! -L \"$1\" ] || exit 1; rm -rf -- \"$1\" || exit 1; fi; rmdir -- \"$2\"",
                "artisan-uninstall",
            ])
            .arg(&versions)
            .arg(&marker.path)
            .spawn()
            .map_err(|_| InstallerError::LifecycleHelper)?;
    }
    Ok(())
}
