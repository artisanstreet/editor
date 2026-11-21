use crate::core::runtime::runtime;
use futures::future::BoxFuture;
use std::hash::{Hash as _, Hasher as _};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::{
    sync::{Arc, LazyLock},
    time::Duration,
};
use tokio_util::task::AbortOnDropHandle;

const MAX_BODY_BYTES: usize = 128 * 1024 * 1024;
const MAX_REDIRECTS: usize = 5;

#[derive(Debug)]
pub struct MediaResponse {
    pub status: reqwest::StatusCode,
    pub body: Vec<u8>,
}

pub trait AddressPolicy: Send + Sync + std::fmt::Debug {
    fn admits(&self, url: &url::Url) -> bool;
}

#[derive(Clone, Copy, Debug)]
pub struct PublicInternet;

impl AddressPolicy for PublicInternet {
    fn admits(&self, url: &url::Url) -> bool {
        public_destination(url)
    }
}

#[derive(Debug)]
pub struct MediaHttpClient {
    http: reqwest::Client,
    policy: Arc<dyn AddressPolicy>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UrlIdentity {
    origin: Option<String>,
    digest: u32,
}

impl UrlIdentity {
    #[must_use]
    fn of(url: &url::Url) -> Self {
        let origin = url.host_str().map(|host| match url.port() {
            Some(port) => format!("{}://{host}:{port}", url.scheme()),
            None => format!("{}://{host}", url.scheme()),
        });
        Self {
            origin,
            digest: digest(url.as_str()),
        }
    }

    #[must_use]
    fn unparsed(text: &str) -> Self {
        Self {
            origin: None,
            digest: digest(text),
        }
    }
}

fn digest(text: &str) -> u32 {
    let mut hasher = std::hash::DefaultHasher::new();
    text.hash(&mut hasher);
    let full = hasher.finish();
    u32::try_from(full >> 32).expect("invariant: the high half of a u64 fits a u32")
}

impl std::fmt::Display for UrlIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.origin {
            Some(origin) => write!(f, "{origin} (#{:08x})", self.digest),
            None => write!(f, "a URL (#{:08x})", self.digest),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RedirectError {
    #[error("{url} redirected without a location")]
    NoLocation { url: UrlIdentity },
    #[error("{url} redirected to a location that is not visible ASCII")]
    UnreadableLocation {
        url: UrlIdentity,
        #[source]
        source: reqwest::header::ToStrError,
    },
    #[error("{url} redirected to an unreadable location")]
    BadLocation {
        url: UrlIdentity,
        #[source]
        source: url::ParseError,
    },
    #[error("{url} redirected to {to}, which is not on the public internet")]
    NotPublic { url: UrlIdentity, to: UrlIdentity },
    #[error("{url} redirected more than {MAX_REDIRECTS} times")]
    TooMany { url: UrlIdentity },
}

#[derive(Debug, thiserror::Error)]
pub enum MediaFetchError {
    #[error("{url} is not a URL")]
    Url {
        url: UrlIdentity,
        #[source]
        source: url::ParseError,
    },
    #[error("{url} is not HTTP or HTTPS")]
    Scheme { url: UrlIdentity },
    #[error("{url} is not on the public internet")]
    NotPublic { url: UrlIdentity },
    #[error("fetch {url}")]
    Fetch {
        url: UrlIdentity,
        #[source]
        source: reqwest::Error,
    },
    #[error("read {url}")]
    Read {
        url: UrlIdentity,
        #[source]
        source: crate::core::client::ReadBodyError,
    },
    #[error("follow a redirect")]
    Redirect(#[source] RedirectError),
    #[error("the fetch task for {url} stopped")]
    Task {
        url: UrlIdentity,
        #[source]
        source: tokio::task::JoinError,
    },
}

#[derive(Debug, thiserror::Error)]
#[error("{host} has no address on the public internet")]
struct NoPublicAddress {
    host: String,
}

struct PublicAddresses;

impl reqwest::dns::Resolve for PublicAddresses {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let public: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await?
                .filter(|address| is_public(address.ip()))
                .collect();
            if public.is_empty() {
                let refused: Box<dyn std::error::Error + Send + Sync> =
                    Box::new(NoPublicAddress { host });
                return Err(refused);
            }
            let addresses: reqwest::dns::Addrs = Box::new(public.into_iter());
            Ok(addresses)
        })
    }
}

fn builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .pool_idle_timeout(Duration::from_secs(90))
        .timeout(Duration::from_secs(30))
}

fn public_client() -> reqwest::Client {
    builder()
        .no_proxy()
        .dns_resolver(Arc::new(PublicAddresses))
        .build()
        .expect("invariant: a rustls client with fixed settings and bundled roots always builds")
}

static SHARED: LazyLock<Arc<MediaHttpClient>> =
    LazyLock::new(|| Arc::new(MediaHttpClient::new(PublicInternet)));

pub fn client() -> Arc<MediaHttpClient> {
    Arc::clone(&SHARED)
}

impl MediaHttpClient {
    #[must_use]
    pub fn new(policy: impl AddressPolicy + 'static) -> Self {
        Self {
            http: public_client(),
            policy: Arc::new(policy),
        }
    }

    pub fn get(
        &self,
        url: &str,
        follow_redirects: bool,
    ) -> BoxFuture<'static, Result<MediaResponse, MediaFetchError>> {
        let client = self.http.clone();
        let policy = Arc::clone(&self.policy);
        let parsed = url::Url::parse(url).map_err(|source| MediaFetchError::Url {
            url: UrlIdentity::unparsed(url),
            source,
        });
        Box::pin(async move {
            let url = parsed?;
            let requested = UrlIdentity::of(&url);
            if !matches!(url.scheme(), "http" | "https") {
                return Err(MediaFetchError::Scheme { url: requested });
            }
            if !policy.admits(&url) {
                return Err(MediaFetchError::NotPublic { url: requested });
            }
            let task = AbortOnDropHandle::new(runtime().spawn(async move {
                let mut url = url;
                for _ in 0..=MAX_REDIRECTS {
                    let response = client.get(url.clone()).send().await.map_err(|source| {
                        MediaFetchError::Fetch {
                            url: UrlIdentity::of(&url),
                            source: source.without_url(),
                        }
                    })?;
                    let status = response.status();
                    if !follow_redirects || !follows(status) {
                        let body = crate::core::client::read_body(response, MAX_BODY_BYTES)
                            .await
                            .map_err(|source| MediaFetchError::Read {
                                url: UrlIdentity::of(&url),
                                source: without_url(source),
                            })?;
                        return Ok(MediaResponse { status, body });
                    }
                    url = next_hop(&url, response.headers(), policy.as_ref())
                        .map_err(MediaFetchError::Redirect)?;
                }
                Err(MediaFetchError::Redirect(RedirectError::TooMany {
                    url: UrlIdentity::of(&url),
                }))
            }));
            task.await.map_err(|source| MediaFetchError::Task {
                url: requested,
                source,
            })?
        })
    }
}

fn without_url(error: crate::core::client::ReadBodyError) -> crate::core::client::ReadBodyError {
    match error {
        crate::core::client::ReadBodyError::Chunk(source) => {
            crate::core::client::ReadBodyError::Chunk(source.without_url())
        }
        too_large @ crate::core::client::ReadBodyError::TooLarge { .. } => too_large,
    }
}

fn follows(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308)
}

fn next_hop(
    url: &url::Url,
    headers: &reqwest::header::HeaderMap,
    policy: &dyn AddressPolicy,
) -> Result<url::Url, RedirectError> {
    let location = headers
        .get(reqwest::header::LOCATION)
        .ok_or_else(|| RedirectError::NoLocation {
            url: UrlIdentity::of(url),
        })?
        .to_str()
        .map_err(|source| RedirectError::UnreadableLocation {
            url: UrlIdentity::of(url),
            source,
        })?;
    let next = url
        .join(location)
        .map_err(|source| RedirectError::BadLocation {
            url: UrlIdentity::of(url),
            source,
        })?;
    if !policy.admits(&next) {
        return Err(RedirectError::NotPublic {
            url: UrlIdentity::of(url),
            to: UrlIdentity::of(&next),
        });
    }
    Ok(next)
}

fn public_destination(url: &url::Url) -> bool {
    matches!(url.scheme(), "http" | "https")
        && match url.host() {
            Some(url::Host::Ipv4(address)) => is_public(IpAddr::V4(address)),
            Some(url::Host::Ipv6(address)) => is_public(IpAddr::V6(address)),
            Some(url::Host::Domain(domain)) => {
                let domain = domain.trim_end_matches('.').to_ascii_lowercase();
                domain != "localhost" && !domain.ends_with(".localhost")
            }
            None => false,
        }
}

fn is_public(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_public_v4(address),
        IpAddr::V6(address) => match V6Route::of(address) {
            V6Route::Native => is_public_v6(address),
            V6Route::Embedded(embedded) => is_public_v4(embedded),
            V6Route::Opaque => false,
        },
    }
}

enum V6Route {
    Native,
    Embedded(Ipv4Addr),
    Opaque,
}

impl V6Route {
    fn of(address: Ipv6Addr) -> Self {
        let v4 = |high: u16, low: u16| {
            let [a, b] = high.to_be_bytes();
            let [c, d] = low.to_be_bytes();
            Ipv4Addr::new(a, b, c, d)
        };
        match address.segments() {
            [0, 0, 0, 0, 0, 0xffff | 0, high, last]
            | [0, 0, 0, 0, 0xffff, 0, high, last]
            | [0x0064, 0xff9b, 0, 0, 0, 0, high, last]
            | [0x2002, high, last, ..] => Self::Embedded(v4(high, last)),
            [0x0064, 0xff9b, 0x0001, ..] | [0x2001, 0, ..] => Self::Opaque,
            _ => Self::Native,
        }
    }
}

fn is_public_v4(address: Ipv4Addr) -> bool {
    let [first, second, third, _] = address.octets();
    !(address.is_unspecified()
        || address.is_loopback()
        || address.is_private()
        || address.is_link_local()
        || address.is_broadcast()
        || address.is_documentation()
        || address.is_multicast()
        || first == 0
        || first >= 240
        || (first == 100 && (64..128).contains(&second))
        || (first == 192 && second == 0 && third == 0)
        || (first == 198 && (18..20).contains(&second)))
}

fn is_public_v6(address: Ipv6Addr) -> bool {
    let [first, second, ..] = address.segments();
    !(address.is_unspecified()
        || address.is_loopback()
        || address.is_multicast()
        || address.is_unique_local()
        || address.is_unicast_link_local()
        || (first & 0xffc0) == 0xfec0
        || (first == 0x2001 && second == 0x0db8))
}
