//! Network access to this installation's Forge from other machines.
//!
//! By default a Forge listens on loopback only. `ae setup --listen ADDRESS
//! --host-name NAME` records a reachable listening address in
//! [`HOST_ACCESS_FILE`]; every start then binds that address and publishes a
//! private host invitation at [`INVITATION_FILE`] once the Forge is ready.
//! An Editor on another machine (or on Windows, for a Forge in WSL) imports
//! that invitation to connect. `auto:PORT` resolves, at each start, to the
//! IPv4 source address of the default route, so a WSL distribution whose
//! address changes across restarts keeps publishing a current endpoint.

use std::{
    fmt, fs,
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket},
    num::NonZeroU16,
    path::{Path, PathBuf},
    str::FromStr,
};

use serde::{Deserialize, Serialize};

use crate::{
    CliError, Result,
    credentials::hosts::{self, HostInvitation},
    instance::{mint_instance_id, write_private_json},
    process::ForgeReadiness,
};

/// Host access configuration inside the installation root.
pub const HOST_ACCESS_FILE: &str = "host-access.json";

/// Private invitation published inside the installation root.
pub const INVITATION_FILE: &str = "host.json";

const SCHEMA: &str = "artisan-host-access-v1";
const MAX_HOST_ACCESS_BYTES: u64 = 4_096;
const MAX_NAME_BYTES: usize = 100;

/// Where the Forge listens for other machines.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ListenAddress {
    /// One concrete, reachable socket address.
    Explicit(SocketAddr),
    /// The IPv4 source address of the default route, resolved at each start.
    DefaultRoute {
        /// Fixed UDP port.
        port: NonZeroU16,
    },
}

impl ListenAddress {
    /// The concrete address to bind now.
    ///
    /// # Errors
    ///
    /// Returns [`CliError::HostAccess`] when no usable default route exists.
    pub fn resolve(self) -> Result<SocketAddr> {
        match self {
            Self::Explicit(address) => Ok(address),
            Self::DefaultRoute { port } => {
                Ok(SocketAddr::new(default_route_address()?, port.get()))
            }
        }
    }
}

impl FromStr for ListenAddress {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        if let Some(port) = value.strip_prefix("auto:") {
            let port = port
                .parse::<NonZeroU16>()
                .map_err(|_| "auto:PORT needs a nonzero port".to_owned())?;
            return Ok(Self::DefaultRoute { port });
        }
        let address = value
            .parse::<SocketAddr>()
            .map_err(|_| "expected IP:PORT or auto:PORT".to_owned())?;
        if address.port() == 0 || !is_reachable_ip(address.ip()) {
            return Err("the address must be a concrete unicast IP with a nonzero port".to_owned());
        }
        Ok(Self::Explicit(address))
    }
}

impl fmt::Display for ListenAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Explicit(address) => write!(formatter, "{address}"),
            Self::DefaultRoute { port } => write!(formatter, "auto:{port}"),
        }
    }
}

fn is_reachable_ip(ip: IpAddr) -> bool {
    !ip.is_unspecified() && !ip.is_multicast() && !matches!(ip, IpAddr::V4(ip) if ip.is_broadcast())
}

/// Selects the default IPv4 route's source address without sending data.
fn default_route_address() -> Result<IpAddr> {
    let unavailable = |_| CliError::HostAccess("no usable default IPv4 route".to_owned());
    let route = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).map_err(unavailable)?;
    // TEST-NET-1: routable in the kernel's view, never contacted.
    route
        .connect((Ipv4Addr::new(192, 0, 2, 1), 9))
        .map_err(unavailable)?;
    let ip = route.local_addr().map_err(unavailable)?.ip();
    if ip.is_loopback() || !is_reachable_ip(ip) {
        return Err(CliError::HostAccess(
            "no usable default IPv4 route; pass an explicit --listen IP:PORT".to_owned(),
        ));
    }
    Ok(ip)
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct HostAccessFile {
    schema: String,
    listen: String,
    name: String,
}

/// This installation's host access: its listening address and the machine
/// name its invitations carry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostAccess {
    listen: ListenAddress,
    name: String,
}

impl HostAccess {
    /// Validates one host access configuration.
    ///
    /// # Errors
    ///
    /// Returns [`CliError::HostAccess`] for an empty, overlong, or control
    /// character name (the rules invitations enforce).
    pub fn new(listen: ListenAddress, name: String) -> Result<Self> {
        if name.trim().is_empty()
            || name.len() > MAX_NAME_BYTES
            || name.chars().any(char::is_control)
        {
            return Err(CliError::HostAccess(format!(
                "the host name must be 1 to {MAX_NAME_BYTES} bytes without control characters"
            )));
        }
        Ok(Self { listen, name })
    }

    /// Where the Forge listens.
    #[must_use]
    pub const fn listen(&self) -> ListenAddress {
        self.listen
    }

    /// The machine name invitations carry.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The configuration file of the installation at `root`.
    #[must_use]
    pub fn path(root: &Path) -> PathBuf {
        root.join(HOST_ACCESS_FILE)
    }

    /// Loads the configuration of the installation at `root`; `None` when
    /// the Forge is loopback-only.
    ///
    /// # Errors
    ///
    /// Returns [`CliError::HostAccess`] for a malformed or oversized file.
    pub fn load(root: &Path) -> Result<Option<Self>> {
        let path = Self::path(root);
        let bytes = match fs::metadata(&path) {
            Ok(metadata) if metadata.is_file() && metadata.len() <= MAX_HOST_ACCESS_BYTES => {
                fs::read(&path).map_err(|source| CliError::Io {
                    context: "read host access",
                    source,
                })?
            }
            Ok(_) => {
                return Err(CliError::HostAccess(format!(
                    "{} is not a bounded regular file",
                    path.display()
                )));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(CliError::Io {
                    context: "inspect host access",
                    source,
                });
            }
        };
        let malformed = || CliError::HostAccess(format!("{} is malformed", path.display()));
        let file: HostAccessFile = serde_json::from_slice(&bytes).map_err(|_| malformed())?;
        if file.schema != SCHEMA {
            return Err(malformed());
        }
        let listen = file.listen.parse().map_err(|_| malformed())?;
        Self::new(listen, file.name).map(Some)
    }

    /// Records this configuration for the installation at `root`.
    ///
    /// # Errors
    ///
    /// Returns [`CliError`] when the file cannot be written privately.
    pub fn write(&self, root: &Path) -> Result<()> {
        write_private_json(
            &Self::path(root),
            &HostAccessFile {
                schema: SCHEMA.to_owned(),
                listen: self.listen.to_string(),
                name: self.name.clone(),
            },
        )
    }

    /// Returns the installation at `root` to loopback-only access, removing
    /// its configuration and any published invitation.
    ///
    /// # Errors
    ///
    /// Returns [`CliError::Io`] when an existing file cannot be removed.
    pub fn remove(root: &Path) -> Result<()> {
        for path in [Self::path(root), root.join(INVITATION_FILE)] {
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(source) => {
                    return Err(CliError::Io {
                        context: "remove host access",
                        source,
                    });
                }
            }
        }
        Ok(())
    }
}

/// Publishes the invitation of the ready Forge `readiness` describes, unless
/// the current invitation already names that Forge process.
///
/// Each Forge start gets a fresh incarnation; the invitation is written
/// through the private credential boundary and renamed into place, so a
/// reader never observes a partial document.
///
/// # Errors
///
/// Returns [`CliError`] when the endpoint is unusable or the invitation
/// cannot be exported.
pub fn publish_invitation(
    root: &Path,
    access: &HostAccess,
    readiness: &ForgeReadiness,
) -> Result<PathBuf> {
    let destination = root.join(INVITATION_FILE);
    if published_for(&destination, readiness.pid()) {
        return Ok(destination);
    }
    let endpoint: SocketAddr = readiness.endpoint().parse().map_err(|_| {
        CliError::HostAccess("the Forge readiness endpoint is not a socket address".to_owned())
    })?;
    let incarnation = mint_instance_id()?;
    let invitation = hosts::invitation_for(
        root,
        access.name().to_owned(),
        endpoint,
        incarnation,
        readiness.pid(),
    )?;
    let export = root.join(format!(".host-export-{}", readiness.pid()));
    let staged = hosts::install_private(&export, INVITATION_FILE, &invitation.encode()?);
    let result = staged.and_then(|staged| {
        fs::rename(&staged, &destination)
            .map_err(|_| crate::credentials::ForgeCredentialError::Provisioning)
    });
    let _ = fs::remove_dir_all(&export);
    result?;
    Ok(destination)
}

/// Whether the invitation at `path` already describes Forge process `pid`.
fn published_for(path: &Path, pid: u32) -> bool {
    fs::read(path)
        .ok()
        .and_then(|bytes| HostInvitation::decode(&bytes).ok())
        .is_some_and(|invitation| invitation.pid == pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listen_addresses_parse_and_render_round_trip() {
        for text in ["auto:4433", "172.29.1.2:4433", "[fd00::2]:4433"] {
            let parsed: ListenAddress = text.parse().unwrap();
            assert_eq!(parsed.to_string(), text);
        }
        for invalid in [
            "auto:0",
            "auto:",
            "0.0.0.0:4433",
            "172.29.1.2:0",
            "224.0.0.1:4433",
            "255.255.255.255:4433",
            "4433",
        ] {
            assert!(invalid.parse::<ListenAddress>().is_err(), "{invalid}");
        }
    }

    #[test]
    fn explicit_addresses_resolve_to_themselves() {
        let address: SocketAddr = "172.29.1.2:4433".parse().unwrap();
        assert_eq!(ListenAddress::Explicit(address).resolve().unwrap(), address);
    }

    #[test]
    fn host_access_round_trips_and_removal_is_idempotent() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(HostAccess::load(root.path()).unwrap(), None);
        let access = HostAccess::new("auto:4433".parse().unwrap(), "Ubuntu".to_owned()).unwrap();
        access.write(root.path()).unwrap();
        assert_eq!(HostAccess::load(root.path()).unwrap(), Some(access));
        fs::write(root.path().join(INVITATION_FILE), b"{}").unwrap();
        HostAccess::remove(root.path()).unwrap();
        HostAccess::remove(root.path()).unwrap();
        assert_eq!(HostAccess::load(root.path()).unwrap(), None);
        assert!(!root.path().join(INVITATION_FILE).exists());
    }

    #[test]
    fn names_follow_the_invitation_rules() {
        let listen = "auto:4433".parse().unwrap();
        for invalid in ["", "  ", "line\nbreak", &"x".repeat(101)] {
            assert!(HostAccess::new(listen, invalid.to_owned()).is_err());
        }
        let root = tempfile::tempdir().unwrap();
        fs::write(
            HostAccess::path(root.path()),
            br#"{"schema":"artisan-host-access-v1","listen":"auto:0","name":"x"}"#,
        )
        .unwrap();
        assert!(HostAccess::load(root.path()).is_err());
    }
}
