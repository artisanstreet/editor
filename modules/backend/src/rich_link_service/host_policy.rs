//! Public-host policy for rich-link URL validation and resolution.
//!
//! A host is public when it is neither `localhost` nor a non-public address
//! literal. DNS names are admitted here on syntax alone because no address
//! exists yet; [`PublicRichLinkResolver`] then resolves the name and refuses
//! any answer that is not publicly routable, returning the validated
//! addresses to the transport. The socket therefore only ever connects to
//! addresses that passed [`is_public_address`], which closes the rebinding
//! gap between the text check and the connection.

#![forbid(unsafe_code)]

use std::{
    error::Error,
    future::Future,
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    pin::Pin,
};

use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use tokio::net::lookup_host;

/// Boxed transport error accepted by [`reqwest::dns::Resolve`].
type BoxError = Box<dyn Error + Send + Sync>;

/// Future returned by one injectable name lookup.
pub(super) type LookupFuture = Pin<Box<dyn Future<Output = io::Result<Vec<SocketAddr>>> + Send>>;

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
        // Domain names carry no address in their text; the pinned resolver
        // rejects any private answer before a connection is attempted.
        Err(_) => true,
    }
}

/// Returns whether one address is globally routable public address space.
///
/// Both families are matched against the IANA special-purpose registries.
/// IPv4-mapped (`::ffff:a.b.c.d`), IPv4-compatible (`::a.b.c.d`, deprecated)
/// and IPv4-translated (`::ffff:0:a.b.c.d`) IPv6 forms are unwrapped and
/// evaluated by the IPv4 rules, so a mapped loopback, private, link-local or
/// metadata address cannot pass as IPv6.
pub(super) fn is_public_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_public_ipv4(address),
        IpAddr::V6(address) => match embedded_ipv4(address) {
            Some(embedded) => is_public_ipv4(embedded),
            None => is_public_ipv6(address),
        },
    }
}

fn is_public_ipv4(address: Ipv4Addr) -> bool {
    if address.is_loopback()
        || address.is_private()
        || address.is_link_local()
        || address.is_unspecified()
        || address.is_broadcast()
        || address.is_documentation()
        || address.is_multicast()
    {
        return false;
    }
    let octets = address.octets();
    // 0.0.0.0/8 "this network" (RFC 1122), 100.64.0.0/10 shared address
    // space (RFC 6598), 192.0.0.0/24 IETF protocol assignments (RFC 6890),
    // 192.88.99.0/24 6to4 relay anycast (RFC 7526), 198.18.0.0/15
    // benchmarking (RFC 2544), and 240.0.0.0/4 reserved (RFC 1112).
    !(octets[0] == 0
        || (octets[0] == 100 && (64..=127).contains(&octets[1]))
        || (octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
        || (octets[0] == 192 && octets[1] == 88 && octets[2] == 99)
        || (octets[0] == 198 && matches!(octets[1], 18 | 19))
        || octets[0] >= 240)
}

fn is_public_ipv6(address: Ipv6Addr) -> bool {
    if address.is_loopback()
        || address.is_unspecified()
        || address.is_multicast()
        || address.is_unique_local()
        || address.is_unicast_link_local()
    {
        return false;
    }
    let segments = address.segments();
    // fec0::/10 deprecated site-local (RFC 3879), 100::/64 discard-only
    // (RFC 6666), 2001::/23 IETF protocol assignments and its benchmarking,
    // ORCHID and Drone Remote ID children (RFC 2928, RFC 4843, RFC 5180,
    // RFC 7343, RFC 9374), 2001:db8::/32 and 3fff::/20 documentation
    // (RFC 3849, RFC 9637), 2002::/16 6to4 (RFC 3056), 5f00::/16 SRv6 SIDs,
    // and 64:ff9b::/96 plus 64:ff9b:1::/48 NAT64 (RFC 6052, RFC 8215).
    !((segments[0] & 0xffc0) == 0xfec0
        || (segments[0] == 0x0100 && segments[1..4] == [0, 0, 0])
        || (segments[0] == 0x2001 && segments[1] <= 0x01ff)
        || (segments[0] == 0x2001 && segments[1] == 0x0db8)
        || segments[0] == 0x2002
        || (segments[0] == 0x3fff && (segments[1] & 0xf000) == 0)
        || segments[0] == 0x5f00
        || (segments[0] == 0x0064
            && segments[1] == 0xff9b
            && (segments[2] == 1 || segments[2..6] == [0, 0, 0, 0])))
}

/// Extracts the embedded IPv4 address from the IPv6 forms that tunnel it:
/// IPv4-mapped `::ffff:a.b.c.d` (RFC 4291), IPv4-compatible `::a.b.c.d`
/// (deprecated RFC 4291), and IPv4-translated `::ffff:0:a.b.c.d` (RFC 2765).
fn embedded_ipv4(address: Ipv6Addr) -> Option<Ipv4Addr> {
    let segments = address.segments();
    let embedded = Ipv4Addr::from((u32::from(segments[6]) << 16) | u32::from(segments[7]));
    if segments[..5] == [0; 5] && matches!(segments[5], 0 | 0xffff) {
        return Some(embedded);
    }
    if segments[..4] == [0; 4] && segments[4] == 0xffff && segments[5] == 0 {
        return Some(embedded);
    }
    None
}

/// One injectable name lookup returning raw socket addresses.
///
/// Production uses the O.S. name service; tests script the answers so the
/// public-address decision stays deterministic without DNS or a network.
pub(super) trait RichLinkNameLookup: Send + Sync + 'static {
    /// Resolves one host name without applying the public-address policy.
    fn lookup(&self, host: String) -> LookupFuture;
}

/// System resolver backed by the O.S. name service through `tokio`.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct SystemRichLinkNameLookup;

impl RichLinkNameLookup for SystemRichLinkNameLookup {
    fn lookup(&self, host: String) -> LookupFuture {
        // Port zero lets `hyper` substitute the scheme's conventional port.
        Box::pin(async move { Ok(lookup_host((host, 0)).await?.collect()) })
    }
}

/// `reqwest::dns::Resolve` implementation that only ever hands back publicly
/// routable addresses.
///
/// Any non-public answer rejects the whole resolution, so a mixed or
/// rebinding response cannot trade on a public sibling.
pub(super) struct PublicRichLinkResolver<L> {
    lookup: L,
}

impl<L: RichLinkNameLookup> PublicRichLinkResolver<L> {
    #[must_use]
    pub(super) fn new(lookup: L) -> Self {
        Self { lookup }
    }
}

impl PublicRichLinkResolver<SystemRichLinkNameLookup> {
    /// Builds the production resolver over the system name service.
    #[must_use]
    pub(super) fn system() -> Self {
        Self::new(SystemRichLinkNameLookup)
    }
}

impl<L: RichLinkNameLookup> Resolve for PublicRichLinkResolver<L> {
    fn resolve(&self, name: Name) -> Resolving {
        let pending = self.lookup.lookup(name.as_str().to_owned());
        Box::pin(async move {
            let answers = pending.await.map_err(boxed_error)?;
            validated_addresses(answers)
        })
    }
}

fn validated_addresses(answers: Vec<SocketAddr>) -> Result<Addrs, BoxError> {
    if answers.is_empty() {
        return Err(Box::new(ResolveError::NoAddresses));
    }
    if answers.iter().any(|answer| !is_public_address(answer.ip())) {
        return Err(Box::new(ResolveError::NonPublicAddress));
    }
    Ok(Box::new(answers.into_iter()))
}

fn boxed_error(error: io::Error) -> BoxError {
    Box::new(error)
}

/// Typed reason a name could not produce a usable public address list.
#[derive(Debug, thiserror::Error)]
enum ResolveError {
    #[error("rich link host resolved to an address that is not publicly routable")]
    NonPublicAddress,
    #[error("rich link host resolved to no addresses")]
    NoAddresses,
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    fn address(input: &str) -> IpAddr {
        input
            .parse()
            .unwrap_or_else(|error| panic!("{input:?} must be an IP literal: {error}"))
    }

    fn socket(input: &str) -> SocketAddr {
        SocketAddr::new(address(input), 0)
    }

    fn name() -> Name {
        Name::from_str("rebind.example").expect("test name parses")
    }

    #[test]
    fn public_policy_blocks_ipv4_special_ranges() {
        for blocked in [
            "0.0.0.0",
            "0.1.2.3",
            "10.0.0.1",
            "100.64.0.1",
            "100.127.255.255",
            "127.0.0.1",
            "127.255.255.255",
            "169.254.169.254",
            "172.16.0.1",
            "172.31.255.255",
            "192.0.0.1",
            "192.0.2.1",
            "192.88.99.1",
            "192.168.1.1",
            "198.18.0.1",
            "198.19.255.255",
            "198.51.100.1",
            "203.0.113.1",
            "224.0.0.1",
            "239.255.255.255",
            "240.0.0.1",
            "255.255.255.254",
            "255.255.255.255",
        ] {
            assert!(
                !is_public_address(address(blocked)),
                "ipv4 {blocked} must be blocked"
            );
        }
    }

    #[test]
    fn public_policy_allows_global_ipv4_unreserved_space() {
        for allowed in [
            "1.1.1.1",
            "8.8.8.8",
            "9.9.9.9",
            "93.184.216.34",
            "100.63.255.255",
            "100.128.0.0",
            "172.15.255.255",
            "172.32.0.0",
            "192.0.1.1",
            "198.17.255.255",
            "198.20.0.0",
            "203.0.114.1",
            "223.255.255.255",
        ] {
            assert!(
                is_public_address(address(allowed)),
                "ipv4 {allowed} must be allowed"
            );
        }
    }

    #[test]
    fn public_policy_unwraps_embedded_ipv4_ipv6_forms() {
        for blocked in [
            "::ffff:127.0.0.1",
            "::ffff:7f00:1",
            "::ffff:169.254.169.254",
            "::ffff:a9fe:a9fe",
            "::ffff:10.0.0.1",
            "::ffff:192.168.0.1",
            "::7f00:1",
            "::a00:1",
            "::ffff:0:7f00:1",
            "::ffff:0:a9fe:a9fe",
            "::ffff:0.0.0.0",
            "::",
            "::1",
        ] {
            assert!(
                !is_public_address(address(blocked)),
                "ipv6 {blocked} must be blocked"
            );
        }
        for allowed in [
            "::ffff:8.8.8.8",
            "::ffff:808:808",
            "::808:808",
            "::ffff:0:808:808",
            "2606:4700:4700::1111",
        ] {
            assert!(
                is_public_address(address(allowed)),
                "ipv6 {allowed} must be allowed"
            );
        }
    }

    #[test]
    fn public_policy_blocks_ipv6_special_ranges() {
        for blocked in [
            "::",
            "::1",
            "100::1",
            "100::ffff:ffff:ffff:ffff",
            "2001::1",
            "2001:2::1",
            "2001:10::1",
            "2001:db8::1",
            "2002:7f00::1",
            "3fff::1",
            "3fff:fff:ffff:ffff:ffff:ffff:ffff:ffff",
            "5f00::1",
            "64:ff9b::7f00:1",
            "64:ff9b:1::1",
            "fc00::1",
            "fd00::1",
            "fe80::1",
            "febf::1",
            "fec0::1",
            "feff::1",
            "ff02::1",
        ] {
            assert!(
                !is_public_address(address(blocked)),
                "ipv6 {blocked} must be blocked"
            );
        }
        for allowed in [
            "2001:4860:4860::8888",
            "2400:cb00::1",
            "2606:4700:4700::1111",
            "2a00:1450:4001:800::200e",
        ] {
            assert!(
                is_public_address(address(allowed)),
                "ipv6 {allowed} must be allowed"
            );
        }
    }

    #[test]
    fn public_host_policy_blocks_localhost_and_literal_private_hosts() {
        for blocked in [
            "localhost",
            "LOCALHOST.",
            "docs.localhost",
            "127.0.0.1",
            "[::1]",
            "[::ffff:127.0.0.1]",
            "[::ffff:7f00:1]",
            "[::ffff:169.254.169.254]",
            "[::ffff:a00:1]",
            "[::ffff:0:7f00:1]",
            "[fd00::1]",
            "[fe80::1]",
            "169.254.169.254",
            "10.1.2.3",
        ] {
            assert!(
                !is_public_rich_link_host(blocked),
                "{blocked} must be blocked"
            );
        }
        for allowed in ["example.com", "EXAMPLE.COM.", "8.8.8.8", "[::ffff:808:808]"] {
            assert!(
                is_public_rich_link_host(allowed),
                "{allowed} must be allowed"
            );
        }
    }

    struct ScriptedLookup {
        answers: Vec<SocketAddr>,
    }

    impl RichLinkNameLookup for ScriptedLookup {
        fn lookup(&self, _host: String) -> LookupFuture {
            let answers = self.answers.clone();
            Box::pin(async move { Ok(answers) })
        }
    }

    #[tokio::test]
    async fn pinned_resolver_returns_validated_public_answers() {
        let answers = vec![socket("93.184.216.34"), socket("2606:4700:4700::1111")];
        let resolver = PublicRichLinkResolver::new(ScriptedLookup {
            answers: answers.clone(),
        });
        let received: Vec<SocketAddr> = resolver
            .resolve(name())
            .await
            .expect("public answers resolve")
            .collect();
        assert_eq!(received, answers);
    }

    #[tokio::test]
    async fn pinned_resolver_rejects_every_non_public_answer() {
        for blocked in [
            "127.0.0.1",
            "169.254.169.254",
            "10.0.0.1",
            "::1",
            "::ffff:127.0.0.1",
            "::ffff:169.254.169.254",
            "fd00::1",
            "fe80::1",
            "2001:db8::1",
            "64:ff9b::7f00:1",
        ] {
            let resolver = PublicRichLinkResolver::new(ScriptedLookup {
                answers: vec![socket(blocked)],
            });
            let outcome = resolver.resolve(name()).await;
            let Err(error) = outcome else {
                panic!("{blocked} must be refused");
            };
            assert!(
                error.downcast_ref::<ResolveError>().is_some(),
                "{blocked} must fail with the typed policy error"
            );
        }
    }

    #[tokio::test]
    async fn pinned_resolver_rejects_mixed_and_empty_answers() {
        let mixed = PublicRichLinkResolver::new(ScriptedLookup {
            answers: vec![socket("93.184.216.34"), socket("127.0.0.1")],
        });
        assert!(
            mixed.resolve(name()).await.is_err(),
            "one private sibling rejects the whole answer set"
        );

        let empty = PublicRichLinkResolver::new(ScriptedLookup {
            answers: Vec::new(),
        });
        assert!(
            empty.resolve(name()).await.is_err(),
            "an empty answer set must be refused"
        );
    }
}
