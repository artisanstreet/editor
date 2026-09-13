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

/// Imports an already trusted invitation under a stable host ID and launch incarnation.
pub fn import(bytes: &[u8]) -> Result<PathBuf, ForgeCredentialError> {
    let invitation = HostInvitation::decode(bytes)?;
    let root = registry_root()?;
    fs::create_dir_all(&root).map_err(|_| ForgeCredentialError::Provisioning)?;
    let epoch = hex(&Sha256::digest(invitation.incarnation));
    let home = root.join(format!("{}-{epoch}", invitation.id()?));
    if home.join("credentials/host.json").exists() {
        let existing = read_private(&home, "host.json")?;
        if existing.as_slice() != invitation.encode()?.as_slice() {
            return Err(ForgeCredentialError::PartialBundle);
        }
    } else {
        install_private(&home, "host.json", &invitation.encode()?)?;
    }
    Ok(home)
}

/// Lists registered host names and private homes, rejecting malformed entries.
pub fn list() -> Result<Vec<(String, PathBuf)>, ForgeCredentialError> {
    let root = registry_root()?;
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err(ForgeCredentialError::Provisioning),
    };
    let mut by_identity = std::collections::BTreeMap::new();
    for entry in entries {
        let entry = entry.map_err(|_| ForgeCredentialError::Provisioning)?;
        let home = entry.path();
        let document = read_private(&home, "host.json")?;
        let invitation = HostInvitation::decode(&document)?;
        let modified = fs::metadata(home.join("credentials/host.json"))
            .and_then(|meta| meta.modified())
            .map_err(|_| ForgeCredentialError::Provisioning)?;
        let entry = by_identity
            .entry(invitation.id()?)
            .or_insert_with(|| (modified, invitation.name.clone(), home.clone()));
        if modified > entry.0 {
            *entry = (modified, invitation.name.clone(), home);
        }
    }
    let mut result: Vec<_> = by_identity
        .into_values()
        .map(|(_, name, home)| (name, home))
        .collect();
    result.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(result)
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

    #[test]
    fn private_registration_refuses_overwrite_and_symlinks() {
        let home = std::env::temp_dir().join(format!(
            "artisan-host-test-{}-{}",
            std::process::id(),
            hex(&crate::instance::mint_instance_id().unwrap())
        ));
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
