//! Explicit remote addressing without weakening the owned-loopback target.
use std::net::{Ipv4Addr, SocketAddr};
use thiserror::Error;

/// Explicit dialing target for either an owned local Forge or a trusted remote host.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionTarget(SocketAddr);

impl SessionTarget {
    /// Validates a concrete unicast endpoint before opening a socket.
    ///
    /// # Errors
    /// Returns an error for unspecified, multicast, broadcast, or zero-port targets.
    pub fn remote(address: SocketAddr) -> Result<Self, SessionTargetError> {
        if address.ip().is_unspecified()
            || address.ip().is_multicast()
            || matches!(address.ip(), std::net::IpAddr::V4(ip) if ip.is_broadcast())
        {
            return Err(SessionTargetError::InvalidRemoteAddress);
        }
        if address.port() == 0 {
            return Err(SessionTargetError::ZeroPort);
        }
        Ok(Self(address))
    }

    /// Returns the validated network address.
    #[must_use]
    pub fn addr(self) -> SocketAddr {
        self.0
    }
}

impl From<LoopbackTarget> for SessionTarget {
    fn from(target: LoopbackTarget) -> Self {
        Self(target.addr())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_remote_targets_do_not_weaken_owned_loopback_validation() {
        for address in [
            "172.29.1.2:4433",
            "192.168.1.50:4433",
            "[::1]:4433",
            "[2001:db8::1]:4433",
        ] {
            let address = address.parse().unwrap();
            assert_eq!(SessionTarget::remote(address).unwrap().addr(), address);
            assert!(LoopbackTarget::new(address).is_err());
        }
        for address in [
            "0.0.0.0:4433",
            "[::]:4433",
            "224.0.0.1:4433",
            "[ff02::1]:4433",
            "255.255.255.255:4433",
            "172.29.1.2:0",
        ] {
            assert!(SessionTarget::remote(address.parse().unwrap()).is_err());
        }
    }
}

/// Exact loopback dialing target validated before any network attempt.
///
/// The transport leaf supports exactly `127.0.0.1` with a nonzero port,
/// matching the loopback bind primitive; everything else is rejected by
/// [`LoopbackTarget::new`] before a socket can exist.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LoopbackTarget(SocketAddr);

/// Why a candidate session target was rejected before any network
/// attempt.
///
/// The discriminants separate an unsupported address from a zero port:
/// loopback spellings this leaf cannot serve (IPv6 `::1`, other
/// `127.x.x.x` addresses) are unsupported addresses, not remote peers.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum SessionTargetError {
    /// A remote endpoint was unspecified, multicast, or broadcast.
    #[error("remote session target must be a concrete unicast address")]
    InvalidRemoteAddress,
    /// The address was not exactly `127.0.0.1`: remote addresses, IPv6
    /// (including `::1`), and every other spelling are unsupported.
    #[error("session target must be exactly 127.0.0.1")]
    UnsupportedAddress,
    /// The target address carried port zero.
    #[error("session target port must be nonzero")]
    ZeroPort,
}

impl LoopbackTarget {
    /// Validates `address` as exactly `127.0.0.1` with a nonzero port.
    ///
    /// The address is diagnosed ahead of the port: a non-loopback
    /// address is unsupported however its port reads.
    ///
    /// # Errors
    ///
    /// Returns [`SessionTargetError::UnsupportedAddress`] for every
    /// address other than IPv4 `127.0.0.1` and
    /// [`SessionTargetError::ZeroPort`] for the loopback address with
    /// port zero.
    pub fn new(address: SocketAddr) -> Result<Self, SessionTargetError> {
        let SocketAddr::V4(v4) = &address else {
            return Err(SessionTargetError::UnsupportedAddress);
        };
        // `SocketAddrV4::ip` hands back a reference; compare through it
        // explicitly so the exact-localhost rule is type-evident.
        if *v4.ip() != Ipv4Addr::LOCALHOST {
            return Err(SessionTargetError::UnsupportedAddress);
        }
        if v4.port() == 0 {
            return Err(SessionTargetError::ZeroPort);
        }
        Ok(Self(address))
    }

    /// Returns the validated target address.
    #[must_use]
    pub fn addr(&self) -> SocketAddr {
        self.0
    }
}
