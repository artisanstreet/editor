//! Trusted Forge host invitations and private local registration.
//!
//! Invitations contain a bootstrap secret and must be transferred through a trusted channel.
//! Only the public leaf certificate is exported; the Forge private key stays on its host.

use super::{ForgeCredentialError, ForgeCredentialPaths, storage};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::{
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
};
use zeroize::Zeroizing;

/// Bound for an invitation, including the DER certificate encoded as base64.
pub const MAX_INVITATION_BYTES: usize = 100_000;

/// One daemon incarnation and its trusted connection material. Debug deliberately absent.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostInvitation {
    /// Format version.
    pub version: u32,
    /// User-visible machine name.
    pub name: String,
    /// Concrete QUIC endpoint.
    pub endpoint: SocketAddr,
    /// Fresh identity generated on each daemon launch.
    pub incarnation: [u8; 16],
    /// Remote daemon process identity, used only as a reconnect fence.
    pub pid: u32,
    /// Base64 public certificate, explicitly trusted during import.
    pub certificate: String,
    /// Base64 initial capability. Never display or log this document.
    pub bootstrap: String,
}

impl Drop for HostInvitation {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.bootstrap);
    }
}

impl HostInvitation {
    /// Decodes and validates all fields before accepting the invitation.
    pub fn decode(bytes: &[u8]) -> Result<Self, ForgeCredentialError> {
        if bytes.len() > MAX_INVITATION_BYTES {
            return Err(ForgeCredentialError::ManifestMalformed);
        }
        let value: Self =
            serde_json::from_slice(bytes).map_err(|_| ForgeCredentialError::ManifestMalformed)?;
        value.validate()?;
        Ok(value)
    }

    /// Validates connection coordinates and credential bounds.
    pub fn validate(&self) -> Result<(), ForgeCredentialError> {
        let invalid = ForgeCredentialError::ManifestMalformed;
        if self.version != 1
            || self.name.trim().is_empty()
            || self.name.len() > 100
            || self.name.chars().any(char::is_control)
            || self.pid == 0
            || self.incarnation == [0; 16]
            || self.endpoint.port() == 0
            || self.endpoint.ip().is_unspecified()
            || self.endpoint.ip().is_multicast()
            || matches!(self.endpoint.ip(), std::net::IpAddr::V4(ip) if ip.is_broadcast())
        {
            return Err(invalid);
        }
        let cert = self.certificate_der()?;
        if cert.is_empty() || cert.len() > 65_536 {
            return Err(invalid);
        }
        let _ = self.capability()?;
        Ok(())
    }

    /// Decodes the trusted public certificate.
    pub fn certificate_der(&self) -> Result<Vec<u8>, ForgeCredentialError> {
        STANDARD
            .decode(&self.certificate)
            .map_err(|_| ForgeCredentialError::ManifestMalformed)
    }

    /// Decodes the secret directly into the protocol's zeroizing owner.
    pub fn capability(&self) -> Result<artisan_protocol::LocalCapability, ForgeCredentialError> {
        let bytes = Zeroizing::new(
            STANDARD
                .decode(&self.bootstrap)
                .map_err(|_| ForgeCredentialError::ManifestMalformed)?,
        );
        artisan_protocol::LocalCapability::try_from_slice(&bytes)
            .map_err(|_| ForgeCredentialError::ManifestMalformed)
    }

    /// Stable host identity is the certificate digest, never its mutable address.
    pub fn id(&self) -> Result<String, ForgeCredentialError> {
        Ok(hex(&Sha256::digest(self.certificate_der()?)))
    }

    /// Encodes a transfer document without exposing it to ordinary diagnostics.
    pub fn encode(&self) -> Result<Zeroizing<Vec<u8>>, ForgeCredentialError> {
        self.validate()?;
        serde_json::to_vec(self)
            .map(Zeroizing::new)
            .map_err(|_| ForgeCredentialError::ManifestMalformed)
    }
}

/// Stores arbitrary host material through the existing private-file boundary.
/// Existing files are never overwritten.
pub fn install_private(
    home: &Path,
    filename: &str,
    bytes: &[u8],
) -> Result<PathBuf, ForgeCredentialError> {
    storage::validate_home(home)?;
    storage::ensure_private_directory(home)?;
    let paths = ForgeCredentialPaths::from_home(home)?;
    storage::ensure_private_directory(&paths.credentials_dir())?;
    storage::install_atomic(&paths.credentials_dir(), filename, bytes, &mut Vec::new())?;
    Ok(paths.credentials_dir().join(filename))
}

/// Reads only a bounded, private, regular file with validated ownership and ancestry.
pub fn read_private(
    home: &Path,
    filename: &str,
) -> Result<Zeroizing<Vec<u8>>, ForgeCredentialError> {
    if !matches!(filename, "host.json" | "source.json") {
        return Err(ForgeCredentialError::ManifestTraversal);
    }
    let paths = ForgeCredentialPaths::from_home(home)?;
    storage::read_private_material(
        &paths,
        &paths.credentials_dir().join(filename),
        MAX_INVITATION_BYTES + 1,
        ForgeCredentialError::ManifestMalformed,
    )
}

/// Returns the platform's per-user host registry without creating it.
pub fn registry_root() -> Result<PathBuf, ForgeCredentialError> {
    let root = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA")
    } else {
        std::env::var_os("XDG_DATA_HOME").or_else(|| {
            std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/share").into_os_string())
        })
    };
    let root = root
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .ok_or(ForgeCredentialError::ManifestMalformed)?;
    Ok(root.join("artisan/hosts"))
}

/// Prefix of a registration directory being retired. Hidden from [`list`].
const RETIRING_PREFIX: &str = ".retiring-";

/// Imports an already trusted invitation under a stable host ID and launch incarnation.
///
/// Once the new registration is installed, registrations of the same host
/// identity from other incarnations are superseded and removed, except any
/// whose reconnect-capability lock a live session still holds.
pub fn import(bytes: &[u8]) -> Result<PathBuf, ForgeCredentialError> {
    import_into(&registry_root()?, bytes)
}

/// Imports the invitation file at `path` and remembers `path` as the host's
/// source, so a later incarnation of the same host can be refreshed from it.
///
/// Only the first import of a registration records its source; the file's
/// bytes are zeroed once imported, since they carry the bootstrap secret.
pub fn import_file(path: &Path) -> Result<PathBuf, ForgeCredentialError> {
    import_file_into(&registry_root()?, path)
}

fn import_file_into(root: &Path, path: &Path) -> Result<PathBuf, ForgeCredentialError> {
    use std::io::Read as _;

    let mut bytes = Zeroizing::new(Vec::new());
    fs::File::open(path)
        .and_then(|file| {
            file.take((MAX_INVITATION_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
        })
        .map_err(|_| ForgeCredentialError::Provisioning)?;
    let home = import_into(root, &bytes)?;
    if !home.join("credentials/source.json").exists() {
        let source =
            serde_json::to_vec(path).map_err(|_| ForgeCredentialError::ManifestMalformed)?;
        install_private(&home, "source.json", &source)?;
    }
    Ok(home)
}

fn import_into(root: &Path, bytes: &[u8]) -> Result<PathBuf, ForgeCredentialError> {
    let invitation = HostInvitation::decode(bytes)?;
    fs::create_dir_all(root).map_err(|_| ForgeCredentialError::Provisioning)?;
    let epoch = hex(&Sha256::digest(invitation.incarnation));
    let id = invitation.id()?;
    let home = root.join(format!("{id}-{epoch}"));
    if home.join("credentials/host.json").exists() {
        let existing = read_private(&home, "host.json")?;
        if existing.as_slice() != invitation.encode()?.as_slice() {
            return Err(ForgeCredentialError::PartialBundle);
        }
    } else {
        install_private(&home, "host.json", &invitation.encode()?)?;
    }
    remove_superseded(root, &id, &home);
    Ok(home)
}

/// Best-effort removal of superseded registrations of host `id`.
///
/// A registration is kept while its reconnect-capability lock is held, since
/// a running Editor session may still own it. Each removable directory is
/// first renamed to a hidden retiring name (which fails on Windows while any
/// file inside is open), so a partial removal is never listed and is retried
/// by the next import.
fn remove_superseded(root: &Path, id: &str, current: &Path) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if let Some(retiring) = name.strip_prefix(RETIRING_PREFIX) {
            if registration_identity(retiring) == Some(id) {
                let _ = fs::remove_dir_all(&path);
            }
            continue;
        }
        if path == current || registration_identity(&name) != Some(id) || session_holds(&path) {
            continue;
        }
        let retiring = root.join(format!("{RETIRING_PREFIX}{name}"));
        if fs::rename(&path, &retiring).is_ok() {
            let _ = fs::remove_dir_all(&retiring);
        }
    }
}

/// Returns whether a live session holds (or may hold) the registration's
/// reconnect-capability lock. Anything that cannot be probed safely is held.
fn session_holds(home: &Path) -> bool {
    let Ok(paths) = ForgeCredentialPaths::from_home(home) else {
        return true;
    };
    if !paths.credentials_dir().exists() {
        return false;
    }
    storage::acquire_lock_with_timeout(&paths.reconnect_lock_path(), std::time::Duration::ZERO)
        .is_err()
}

/// Returns the host identity of a registration directory named `<id>-<epoch>`.
fn registration_identity(name: &str) -> Option<&str> {
    let (id, epoch) = name.split_once('-')?;
    let digest = |text: &str| text.len() == 64 && text.bytes().all(|b| b.is_ascii_hexdigit());
    (digest(id) && digest(epoch)).then_some(id)
}

/// Returns `home` while it is registered, otherwise the newest registration
/// of the same host identity that superseded it. Unknown homes are returned
/// unchanged so callers report their own missing-registration failure.
pub fn current_home(home: &Path) -> PathBuf {
    if home.join("credentials/host.json").exists() {
        return home.to_path_buf();
    }
    let id = home
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(registration_identity);
    let (Some(root), Some(id)) = (home.parent(), id) else {
        return home.to_path_buf();
    };
    registrations(root)
        .into_iter()
        .find(|registration| registration.id == id)
        .map_or_else(|| home.to_path_buf(), |registration| registration.home)
}

/// Lists registered host names and private homes, newest registration per identity.
///
/// Entries that are not complete, valid registrations (half-written, being
/// retired, or foreign files) are skipped rather than failing the listing.
pub fn list() -> Result<Vec<(String, PathBuf)>, ForgeCredentialError> {
    let root = registry_root()?;
    match fs::metadata(&root) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err(ForgeCredentialError::Provisioning),
    }
    let mut result: Vec<_> = registrations(&root)
        .into_iter()
        .map(|registration| (registration.name, registration.home))
        .collect();
    result.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(result)
}

struct Registration {
    id: String,
    name: String,
    home: PathBuf,
}

/// Newest valid registration per host identity under `root`.
fn registrations(root: &Path) -> Vec<Registration> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut by_identity = std::collections::BTreeMap::new();
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let home = entry.path();
        let Ok(invitation) =
            read_private(&home, "host.json").and_then(|document| HostInvitation::decode(&document))
        else {
            continue;
        };
        let (Ok(id), Ok(modified)) = (
            invitation.id(),
            fs::metadata(home.join("credentials/host.json")).and_then(|meta| meta.modified()),
        ) else {
            continue;
        };
        let newest = by_identity
            .entry(id.clone())
            .or_insert_with(|| (modified, invitation.name.clone(), home.clone()));
        if modified > newest.0 {
            *newest = (modified, invitation.name.clone(), home);
        }
    }
    by_identity
        .into_iter()
        .map(|(id, (_, name, home))| Registration { id, name, home })
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut text, byte| {
            write!(&mut text, "{byte:02x}").expect("String write");
            text
        })
}

/// Exports only client material from a provisioned Forge home.
pub fn invitation_for(
    home: &Path,
    name: String,
    endpoint: SocketAddr,
    incarnation: [u8; 16],
    pid: u32,
) -> Result<HostInvitation, ForgeCredentialError> {
    let (certificate, _) = super::load_client_credentials(home)?.into_parts();
    let paths = ForgeCredentialPaths::from_home(home)?;
    let capability = storage::read_private_material(
        &paths,
        paths.capability_path(),
        33,
        ForgeCredentialError::ManifestMalformed,
    )?;
    let invitation = HostInvitation {
        version: 1,
        name,
        endpoint,
        incarnation,
        pid,
        certificate: STANDARD.encode(certificate.as_ref()),
        bootstrap: STANDARD.encode(capability.as_slice()),
    };
    invitation.validate()?;
    Ok(invitation)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invitation() -> HostInvitation {
        HostInvitation {
            version: 1,
            name: "Ubuntu".into(),
            endpoint: "172.29.1.2:4433".parse().unwrap(),
            incarnation: [1; 16],
            pid: 42,
            certificate: STANDARD.encode([1, 2, 3]),
            bootstrap: STANDARD.encode([7; 32]),
        }
    }

    #[test]
    fn identity_survives_address_and_incarnation_changes() {
        let mut host = invitation();
        let id = host.id().unwrap();
        host.endpoint = "172.30.1.2:4433".parse().unwrap();
        host.incarnation = [2; 16];
        assert_eq!(host.id().unwrap(), id);
        host.certificate = STANDARD.encode([4, 5, 6]);
        assert_ne!(host.id().unwrap(), id);
    }

    #[test]
    fn rejects_invalid_invitations_before_storage() {
        let mut host = invitation();
        assert!(HostInvitation::decode(&host.encode().unwrap()).is_ok());
        host.endpoint = "0.0.0.0:4433".parse().unwrap();
        assert!(host.validate().is_err());
        host.endpoint = "172.29.1.2:4433".parse().unwrap();
        host.bootstrap = STANDARD.encode([7; 31]);
        assert!(host.validate().is_err());
        assert!(HostInvitation::decode(&vec![b' '; MAX_INVITATION_BYTES + 1]).is_err());
    }

    fn temporary_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "artisan-{label}-{}-{}",
            std::process::id(),
            hex(&crate::instance::mint_instance_id().unwrap())
        ))
    }

    fn incarnation(value: u8) -> Vec<u8> {
        let mut host = invitation();
        host.incarnation = [value; 16];
        host.encode().unwrap().to_vec()
    }

    #[test]
    fn import_retires_superseded_incarnations_except_live_sessions() {
        let root = temporary_root("hosts-supersede");
        let mut other = invitation();
        other.certificate = STANDARD.encode([4, 5, 6]);
        let unrelated = import_into(&root, &other.encode().unwrap()).unwrap();

        let first = import_into(&root, &incarnation(1)).unwrap();
        let second = import_into(&root, &incarnation(2)).unwrap();
        assert!(!first.exists(), "the superseded incarnation is removed");
        assert!(second.join("credentials/host.json").exists());

        // A live Editor session holds the second registration's reconnect lock.
        let paths = ForgeCredentialPaths::from_home(&second).unwrap();
        let session = storage::acquire_lock(&paths.reconnect_lock_path()).unwrap();
        let third = import_into(&root, &incarnation(3)).unwrap();
        assert!(second.exists(), "a locked registration is never removed");
        assert_eq!(current_home(&first), third);
        assert_eq!(current_home(&second), second);

        // Once the session ends, re-importing the current invitation retires it.
        drop(session);
        assert_eq!(import_into(&root, &incarnation(3)).unwrap(), third);
        assert!(!second.exists());
        assert_eq!(current_home(&second), third);
        assert!(unrelated.join("credentials/host.json").exists());
        let listed: Vec<_> = registrations(&root).into_iter().map(|r| r.home).collect();
        assert_eq!(listed.len(), 2);
        assert!(listed.contains(&third) && listed.contains(&unrelated));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn listing_skips_half_written_retiring_and_foreign_entries() {
        let root = temporary_root("hosts-list");
        let home = import_into(&root, &incarnation(1)).unwrap();
        let name = home.file_name().unwrap().to_str().unwrap().to_owned();
        // A half-written registration of a newer incarnation: no host.json.
        let half = root.join(format!("{}-{}", &name[..64], "0".repeat(64)));
        fs::create_dir_all(half.join("credentials")).unwrap();
        // An interrupted retirement that still holds a complete copy.
        let retiring = root.join(format!(
            "{RETIRING_PREFIX}{}-{}",
            &name[..64],
            "1".repeat(64)
        ));
        install_private(&retiring, "host.json", &incarnation(9)).unwrap();
        fs::create_dir_all(root.join("unrelated-directory")).unwrap();
        fs::write(root.join("stray-file"), b"not a registration").unwrap();
        fs::create_dir_all(root.join("corrupt/credentials")).unwrap();
        fs::write(root.join("corrupt/credentials/host.json"), b"{").unwrap();

        let listed = registrations(&root);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].home, home);
        assert_eq!(current_home(&half), home);

        // The next import finishes the interrupted retirement and removes the
        // half-written registration, which no session can hold.
        import_into(&root, &incarnation(1)).unwrap();
        assert!(!retiring.exists() && !half.exists());
        assert!(root.join("unrelated-directory").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn file_import_records_its_source_once() {
        let root = temporary_root("hosts-file");
        let invitation = root.join("invitation.json");
        fs::create_dir_all(&root).unwrap();
        fs::write(&invitation, incarnation(1)).unwrap();
        let home = import_file_into(&root.join("hosts"), &invitation).unwrap();
        let recorded: PathBuf =
            serde_json::from_slice(&read_private(&home, "source.json").unwrap()).unwrap();
        assert_eq!(recorded, invitation);

        // A newer incarnation from another path is a new registration with
        // its own source; the superseded one is retired.
        let moved = root.join("moved.json");
        fs::write(&moved, incarnation(2)).unwrap();
        let next = import_file_into(&root.join("hosts"), &moved).unwrap();
        assert_ne!(next, home);
        let recorded: PathBuf =
            serde_json::from_slice(&read_private(&next, "source.json").unwrap()).unwrap();
        assert_eq!(recorded, moved);
        assert!(!home.exists());
        assert!(import_file_into(&root.join("hosts"), &root.join("absent.json")).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn private_registration_refuses_overwrite_and_symlinks() {
        let home = temporary_root("host-test");
        let encoded = invitation().encode().unwrap();
        install_private(&home, "host.json", &encoded).unwrap();
        assert!(install_private(&home, "host.json", &encoded).is_err());
        assert_eq!(
            read_private(&home, "host.json").unwrap().as_slice(),
            encoded.as_slice()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::{PermissionsExt as _, symlink};
            let file = home.join("credentials/host.json");
            assert_eq!(
                fs::metadata(&file).unwrap().permissions().mode() & 0o777,
                0o600
            );
            fs::rename(&file, home.join("original.json")).unwrap();
            symlink(home.join("original.json"), file).unwrap();
            assert!(read_private(&home, "host.json").is_err());
        }
        fs::remove_dir_all(home).unwrap();
    }
}
