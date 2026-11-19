use editor_core::media_http::AddressPolicy;
use std::net::{IpAddr, SocketAddr};

#[derive(Debug)]
pub struct LoopbackStubs(pub Vec<SocketAddr>);

impl AddressPolicy for LoopbackStubs {
    fn admits(&self, url: &url::Url) -> bool {
        let address = match url.host() {
            Some(url::Host::Ipv4(address)) => IpAddr::V4(address),
            Some(url::Host::Ipv6(address)) => IpAddr::V6(address),
            Some(url::Host::Domain(_)) | None => return false,
        };
        url.port_or_known_default()
            .is_some_and(|port| self.0.contains(&SocketAddr::new(address, port)))
    }
}
