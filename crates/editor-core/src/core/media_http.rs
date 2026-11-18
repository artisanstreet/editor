use crate::core::runtime::runtime;
use futures::future::BoxFuture;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::{sync::Arc, time::Duration};
use tokio_util::task::AbortOnDropHandle;

const MAX_BODY_BYTES: usize = 128 * 1024 * 1024;
const MAX_REDIRECTS: usize = 5;

pub struct MediaResponse {
    pub status: reqwest::StatusCode,
    pub body: Vec<u8>,
}

pub struct MediaHttpClient {
    first: reqwest::Client,
    onward: reqwest::Client,
}

#[derive(Debug, thiserror::Error)]
pub enum RedirectError {
    #[error("{url} redirected without a readable location")]
    NoLocation { url: String },
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

pub fn client() -> Arc<MediaHttpClient> {
    let builder = || {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .pool_idle_timeout(None)
            .timeout(Duration::from_secs(30))
    };
    Arc::new(MediaHttpClient {
        first: builder().build().expect("build public media HTTP client"),
        onward: builder()
            .no_proxy()
            .dns_resolver(Arc::new(PublicAddresses))
            .build()
            .expect("build public media redirect HTTP client"),
    })
}

impl MediaHttpClient {
    pub fn get(
        &self,
        url: &str,
        follow_redirects: bool,
    ) -> BoxFuture<'static, Result<MediaResponse, MediaFetchError>> {
        let first = self.first.clone();
        let onward = self.onward.clone();
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
            let task = AbortOnDropHandle::new(runtime().spawn(async move {
                let mut url = url;
                let mut client = first;
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
                    url = next_hop(&url, response.headers()).map_err(MediaFetchError::Redirect)?;
                    client = onward.clone();
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
) -> Result<url::Url, RedirectError> {
    let location = headers
        .get(reqwest::header::LOCATION)
        .and_then(|location| location.to_str().ok())
        .ok_or_else(|| RedirectError::NoLocation {
            url: url.to_string(),
        })?;
    let next = url
        .join(location)
        .map_err(|source| RedirectError::BadLocation {
            url: url.to_string(),
            source,
        })?;
    let public = matches!(next.scheme(), "http" | "https")
        && match next.host() {
            Some(url::Host::Ipv4(address)) => is_public(IpAddr::V4(address)),
            Some(url::Host::Ipv6(address)) => is_public(IpAddr::V6(address)),
            Some(url::Host::Domain(domain)) => {
                let domain = domain.trim_end_matches('.').to_ascii_lowercase();
                domain != "localhost" && !domain.ends_with(".localhost")
            }
            None => false,
        };
    if !public {
        return Err(RedirectError::NotPublic {
            url: url.to_string(),
            to: next.to_string(),
        });
    }
    Ok(next)
}

fn is_public(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_public_v4(address),
        IpAddr::V6(address) => address
            .to_ipv4_mapped()
            .map_or_else(|| is_public_v6(address), is_public_v4),
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
