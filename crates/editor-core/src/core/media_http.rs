use crate::core::runtime::runtime;
use futures::future::BoxFuture;
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

#[derive(Debug, thiserror::Error)]
pub enum RedirectError {
    #[error("{url} redirected without a location")]
    NoLocation { url: String },
    #[error("{url} redirected to a location that is not visible ASCII")]
    UnreadableLocation {
        url: String,
        #[source]
        source: reqwest::header::ToStrError,
    },
    #[error("{url} redirected to an unreadable location")]
    BadLocation {
        url: String,
        #[source]
        source: url::ParseError,
    },
    #[error("{url} redirected to {to}, which is not on the public internet")]
    NotPublic { url: String, to: String },
    #[error("{url} redirected more than {MAX_REDIRECTS} times")]
    TooMany { url: String },
}

#[derive(Debug, thiserror::Error)]
pub enum MediaFetchError {
    #[error("{url} is not a URL")]
    Url {
        url: String,
        #[source]
        source: url::ParseError,
    },
    #[error("{url} is not HTTP or HTTPS")]
    Scheme { url: String },
    #[error("{url} is not on the public internet")]
    NotPublic { url: String },
    #[error("fetch {url}")]
    Fetch {
        url: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("read {url}")]
    Read {
        url: String,
        #[source]
        source: crate::core::client::ReadBodyError,
    },
    #[error("follow a redirect")]
    Redirect(#[source] RedirectError),
    #[error("the fetch task for {url} stopped")]
    Task {
        url: String,
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
        .expect("build public media HTTP client")
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
        let requested = url.to_owned();
        let parsed = url::Url::parse(url);
        Box::pin(async move {
            let url = parsed.map_err(|source| MediaFetchError::Url {
                url: requested.clone(),
                source,
            })?;
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
                            url: url.to_string(),
                            source,
                        }
                    })?;
                    let status = response.status();
                    if !follow_redirects || !follows(status) {
                        let body = crate::core::client::read_body(response, MAX_BODY_BYTES)
                            .await
                            .map_err(|source| MediaFetchError::Read {
                                url: url.to_string(),
                                source,
                            })?;
                        return Ok(MediaResponse { status, body });
                    }
                    url = next_hop(&url, response.headers(), policy.as_ref())
                        .map_err(MediaFetchError::Redirect)?;
                }
                Err(MediaFetchError::Redirect(RedirectError::TooMany {
                    url: url.to_string(),
                }))
            }));
            task.await.map_err(|source| MediaFetchError::Task {
                url: requested,
                source,
            })?
        })
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
            url: url.to_string(),
        })?
        .to_str()
        .map_err(|source| RedirectError::UnreadableLocation {
            url: url.to_string(),
            source,
        })?;
    let next = url
        .join(location)
        .map_err(|source| RedirectError::BadLocation {
            url: url.to_string(),
            source,
        })?;
    if !policy.admits(&next) {
        return Err(RedirectError::NotPublic {
            url: url.to_string(),
            to: next.to_string(),
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
            V6Route::LocalTranslation => false,
        },
    }
}

enum V6Route {
    Native,
    Embedded(Ipv4Addr),
    LocalTranslation,
}

impl V6Route {
    fn of(address: Ipv6Addr) -> Self {
        let v4 = |high: u16, low: u16| {
            let [a, b] = high.to_be_bytes();
            let [c, d] = low.to_be_bytes();
            Ipv4Addr::new(a, b, c, d)
        };
        match address.segments() {
            [0, 0, 0, 0, 0, 0xffff, high, last] | [0x0064, 0xff9b, 0, 0, 0, 0, high, last] => {
                Self::Embedded(v4(high, last))
            }
            [0x0064, 0xff9b, 0x0001, ..] => Self::LocalTranslation,
            [0x2002, high, last, ..] => Self::Embedded(v4(high, last)),
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
