//! Public-host policy for rich-link URL validation.
//!
//! A host is public when it is neither `localhost` nor a non-public IP
//! literal. DNS names are validated only as syntax here: the reference pins
//! resolved addresses too, while this build relies on the O.S. resolver and
//! records that gap as remaining uncertainty.

#![forbid(unsafe_code)]

use std::net::IpAddr;

pub(super) fn is_public_rich_link_host(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() || host == "localhost" || host.ends_with(".localhost") {
        return false;
    }
    let literal = host
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(&host);
    match literal.parse::<IpAddr>() {
        Ok(address) => is_public_address(address),
        // DNS names are validated only as syntax here. The reference pins
        // resolved addresses too; this build relies on the O.S. resolver and
        // records that gap as remaining uncertainty.
        Err(_) => true,
    }
}

pub(super) fn is_public_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            let octets = address.octets();
            !(address.is_loopback()
                || address.is_private()
                || address.is_link_local()
                || address.is_unspecified()
                || address.is_broadcast()
                || address.is_documentation()
                || address.is_multicast()
                || octets[0] == 0
                || (octets[0] == 100 && (64..=127).contains(&octets[1]))
                || (octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
                || (octets[0] == 198 && (18..=19).contains(&octets[1])))
        }
        IpAddr::V6(address) => {
            !(address.is_loopback()
                || address.is_unspecified()
                || address.is_multicast()
                || address.is_unique_local()
                || address.is_unicast_link_local())
        }
    }
}
