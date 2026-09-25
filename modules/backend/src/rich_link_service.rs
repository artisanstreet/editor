//! Bounded outbound rich-link metadata resolution.
//!
//! Assistant-authored `http(s)` links present the target page's resolved title
//! in place of the authored label. This module owns the one outbound fetch
//! that produces that title: a small HTML metadata parser, a size- and
//! time-bounded HTTP transport, and a bounded TTL cache that shares one
//! in-flight fetch per canonical URL.
//!
//! The parser follows the reference metadata precedence: `og:title`, then
//! `twitter:title`, then the document `<title>`, then the canonical host as
//! the site-name fallback. Only `text/html` and `application/xhtml+xml`
//! responses are metadata documents; every other content type fails and the
//! caller keeps the authored label.
//!
//! No path, credential, or raw HTML byte leaves this module: the only outputs
//! are bounded display text, typed failures, and the cache expiry instant.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    pin::Pin,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use reqwest::{Url, header::CONTENT_TYPE};
use tokio::{
    sync::{Mutex, watch},
    time::timeout,
};

mod host_policy;
mod html;

pub use html::{normalize_rich_link_text, parse_rich_link_html};

use host_policy::{PublicRichLinkResolver, is_public_rich_link_host};

/// Maximum metadata characters retained from one document text field.
///
/// The reference bounds each normalized metadata value to 512 Unicode scalar
/// values; this ceiling rides the same policy so a hostile document cannot
/// force unbounded display text into the transcript.
pub const RICH_LINK_PAGE_NAME_MAX_CHARS: usize = 512;

/// Default maximum HTML bytes read from one target.
pub const RICH_LINK_DEFAULT_MAX_HTML_BYTES: usize = 512 * 1024;

/// Default number of resolved URLs retained by one resolver cache.
pub const RICH_LINK_DEFAULT_CACHE_CAPACITY: usize = 64;

/// Default cache time-to-live for one resolved URL.
pub const RICH_LINK_DEFAULT_CACHE_TTL: Duration = Duration::from_mins(5);

/// Default TCP connect timeout for one outbound request.
pub const RICH_LINK_DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// Default total response timeout for one outbound request.
pub const RICH_LINK_DEFAULT_RESPONSE_TIMEOUT: Duration = Duration::from_secs(7);

/// Default redirect limit for one outbound request.
pub const RICH_LINK_DEFAULT_MAX_REDIRECTS: usize = 5;

/// Finite, payload-free failure for one rich-link resolution.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum RichLinkError {
    /// The target is not an absolute HTTP(S) URL without credentials.
    #[error("rich link url is not an absolute HTTP(S) destination")]
    InvalidUrl,
    /// The target host is localhost or a non-public address literal.
    #[error("rich link address is not publicly routable")]
    BlockedAddress,
    /// The response is not an HTML metadata document.
    #[error("rich link target is not an HTML page")]
    UnsupportedContentType,
    /// The response status was outside the successful range.
    #[error("rich link target returned an unexpected HTTP status")]
    HttpStatus,
    /// The response body exceeded the configured HTML byte bound.
    #[error("rich link target exceeded its HTML byte bound")]
    ResponseTooLarge,
    /// The bounded fetch deadline elapsed.
    #[error("rich link resolution timed out")]
    Timeout,
    /// The transport failed before a complete response was read.
    #[error("rich link transport failed")]
    Transport,
    /// The resolver or fetcher was configured with invalid limits.
    #[error("rich link resolution is misconfigured")]
    Configuration,
    /// The in-flight owner disappeared before publishing a result.
    #[error("rich link resolution owner is unavailable")]
    Unavailable,
}

/// One resolved display title with its backend cache expiry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RichLinkResolution {
    /// Optional bounded favicon image bytes. Empty when unavailable.
    pub favicon: Vec<u8>,
    /// Exact canonical URL the resolution answers.
    pub requested_url: String,
    /// Resolved display title, already bounded and non-empty.
    pub page_name: String,
    /// Absolute Unix epoch millisecond expiry of the backing cache entry.
    pub expires_at_ms: i64,
}

/// Limits for one [`RichLinkResolver`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RichLinkResolverOptions {
    /// How long one resolved title stays fresh.
    pub cache_ttl: Duration,
    /// Maximum number of retained resolved URLs.
    pub cache_capacity: usize,
    /// Maximum HTML bytes accepted from one target.
    pub max_html_bytes: usize,
    /// Total deadline for one outbound fetch.
    pub fetch_timeout: Duration,
}

impl Default for RichLinkResolverOptions {
    fn default() -> Self {
        Self {
            cache_ttl: RICH_LINK_DEFAULT_CACHE_TTL,
            cache_capacity: RICH_LINK_DEFAULT_CACHE_CAPACITY,
            max_html_bytes: RICH_LINK_DEFAULT_MAX_HTML_BYTES,
            fetch_timeout: RICH_LINK_DEFAULT_CONNECT_TIMEOUT + RICH_LINK_DEFAULT_RESPONSE_TIMEOUT,
        }
    }
}

impl RichLinkResolverOptions {
    /// Validates every limit before a resolver consumes it.
    ///
    /// # Errors
    ///
    /// Returns [`RichLinkError::Configuration`] for a zero TTL, capacity, or
    /// byte bound, or a zero fetch deadline.
    pub fn validate(self) -> Result<Self, RichLinkError> {
        if self.cache_ttl.is_zero()
            || self.cache_capacity == 0
            || self.max_html_bytes == 0
            || self.fetch_timeout.is_zero()
        {
            return Err(RichLinkError::Configuration);
        }
        Ok(self)
    }
}

/// One fetched candidate HTML document.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RichLinkPage {
    /// Final URL after redirects.
    pub final_url: String,
    /// Lowercased media type without parameters.
    pub content_type: String,
    /// Raw response bytes.
    pub body: Vec<u8>,
}

/// Boxed future returned by one rich-link page fetch.
pub type RichLinkFetchFuture =
    Pin<Box<dyn Future<Output = Result<RichLinkPage, RichLinkError>> + Send>>;

/// Narrow outbound seam so tests can script pages without a network.
pub trait RichLinkPageFetcher: Send + Sync + 'static {
    /// Fetches one already-validated absolute HTTP(S) URL.
    fn fetch(&self, url: Url) -> RichLinkFetchFuture;
    /// Fetches an optional icon through the same public-address policy.
    fn fetch_icon(&self, _url: Url) -> Pin<Box<dyn Future<Output = Vec<u8>> + Send>> {
        Box::pin(async { Vec::new() })
    }
}

/// Monotonic-enough wall clock seam for cache expiry tests.
pub trait RichLinkClock: Send + Sync + 'static {
    /// Returns the current Unix epoch millisecond instant.
    fn now_ms(&self) -> i64;
}

/// Production clock reading the system wall clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemRichLinkClock;

impl RichLinkClock for SystemRichLinkClock {
    fn now_ms(&self) -> i64 {
        i64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_millis()),
        )
        .unwrap_or(i64::MAX)
    }
}

/// Limits for the production HTTP fetcher.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HttpRichLinkFetcherOptions {
    /// TCP connect timeout.
    pub connect_timeout: Duration,
    /// Total response timeout.
    pub response_timeout: Duration,
    /// Maximum HTML bytes read before failing.
    pub max_html_bytes: usize,
    /// Maximum redirects followed before failing.
    pub max_redirects: usize,
}

impl Default for HttpRichLinkFetcherOptions {
    fn default() -> Self {
        Self {
            connect_timeout: RICH_LINK_DEFAULT_CONNECT_TIMEOUT,
            response_timeout: RICH_LINK_DEFAULT_RESPONSE_TIMEOUT,
            max_html_bytes: RICH_LINK_DEFAULT_MAX_HTML_BYTES,
            max_redirects: RICH_LINK_DEFAULT_MAX_REDIRECTS,
        }
    }
}

/// Production reqwest-based fetcher with bounded size, time, and redirects.
pub struct HttpRichLinkFetcher {
    client: Result<reqwest::Client, RichLinkError>,
    options: HttpRichLinkFetcherOptions,
}

impl HttpRichLinkFetcher {
    /// Builds one fetcher. Construction never panics: a client build failure
    /// is retained as [`RichLinkError::Configuration`] and surfaced by the
    /// first fetch.
    #[must_use]
    pub fn new(options: HttpRichLinkFetcherOptions) -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(options.connect_timeout)
            .redirect(redirect_policy(options.max_redirects))
            .user_agent("Artisan rich-link preview")
            // Proxy resolution would move name lookup off this process and out
            // of reach of the pinned resolver, so the fetch stays direct.
            .no_proxy()
            .dns_resolver(Arc::new(PublicRichLinkResolver::system()))
            .build()
            .map_err(|_| RichLinkError::Configuration);
        Self { client, options }
    }

    /// Builds the production fetcher with reference-equivalent limits.
    #[must_use]
    pub fn with_defaults() -> Self {
        Self::new(HttpRichLinkFetcherOptions::default())
    }

    async fn get(&self, url: Url) -> Result<RichLinkPage, RichLinkError> {
        let client = self.client.as_ref().map_err(|error| *error)?.clone();
        let mut response = client
            .get(url)
            .header(reqwest::header::ACCEPT, "text/html, application/xhtml+xml")
            .send()
            .await
            .map_err(|error| map_reqwest_error(&error))?;
        if !response.status().is_success() {
            return Err(RichLinkError::HttpStatus);
        }
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map_or(String::new(), |value| {
                value
                    .split(';')
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .to_ascii_lowercase()
            });
        if !is_html_content_type(&content_type) {
            return Err(RichLinkError::UnsupportedContentType);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| map_reqwest_error(&error))?
        {
            if body.len().saturating_add(chunk.len()) > self.options.max_html_bytes {
                return Err(RichLinkError::ResponseTooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(RichLinkPage {
            final_url: response.url().as_str().to_owned(),
            content_type,
            body,
        })
    }
}

impl RichLinkPageFetcher for HttpRichLinkFetcher {
    fn fetch_icon(&self, url: Url) -> Pin<Box<dyn Future<Output = Vec<u8>> + Send>> {
        let client = self.client.clone();
        Box::pin(async move {
            let fetch = async {
                let url = canonical_rich_link_url(url.as_str()).ok()?;
                let mut response = client.ok()?.get(url).send().await.ok()?;
                if !response.status().is_success() {
                    return None;
                }
                let mut bytes = Vec::new();
                while let Some(chunk) = response.chunk().await.ok()? {
                    if bytes.len().saturating_add(chunk.len()) > 65_536 {
                        return None;
                    }
                    bytes.extend_from_slice(&chunk);
                }
                Some(bytes)
            };
            timeout(Duration::from_secs(2), fetch)
                .await
                .ok()
                .flatten()
                .unwrap_or_default()
        })
    }

    fn fetch(&self, url: Url) -> RichLinkFetchFuture {
        let fetcher = Self {
            client: self.client.clone(),
            options: self.options,
        };
        let deadline = self.options.connect_timeout + self.options.response_timeout;
        Box::pin(async move {
            match timeout(deadline, fetcher.get(url)).await {
                Ok(result) => result,
                Err(_) => Err(RichLinkError::Timeout),
            }
        })
    }
}

/// Bounds redirect hops and refuses non-public redirect targets.
///
/// This is the per-hop text gate; the pinned resolver independently
/// re-validates every name at connection time, so a hop must pass both.
fn redirect_policy(max_redirects: usize) -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() >= max_redirects {
            return attempt.error("rich link redirect limit exceeded");
        }
        if canonical_rich_link_url(attempt.url().as_str()).is_err() {
            return attempt.error("rich link redirect target is not publicly routable");
        }
        attempt.follow()
    })
}

fn map_reqwest_error(error: &reqwest::Error) -> RichLinkError {
    if error.is_timeout() {
        RichLinkError::Timeout
    } else if error.is_redirect() || error.is_status() {
        RichLinkError::HttpStatus
    } else {
        RichLinkError::Transport
    }
}

fn is_html_content_type(content_type: &str) -> bool {
    matches!(content_type, "text/html" | "application/xhtml+xml")
}

/// Parses and canonicalizes one assistant-supplied URL for resolution.
///
/// # Errors
///
/// Returns [`RichLinkError::InvalidUrl`] for malformed, relative, credential,
/// or non-HTTP(S) targets, and [`RichLinkError::BlockedAddress`] when the host
/// is localhost or a non-public IP literal. Fragments are stripped exactly
/// like the reference canonical URL.
pub fn canonical_rich_link_url(input: &str) -> Result<Url, RichLinkError> {
    let mut url = Url::parse(input).map_err(|_| RichLinkError::InvalidUrl)?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(RichLinkError::InvalidUrl);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(RichLinkError::InvalidUrl);
    }
    if !is_public_rich_link_host(url.host_str().unwrap_or_default()) {
        return Err(RichLinkError::BlockedAddress);
    }
    url.set_fragment(None);
    Ok(url)
}

#[derive(Clone)]
struct RichLinkCacheEntry {
    favicon: Vec<u8>,
    page_name: String,
    expires_at_ms: i64,
}

type FlightOutcome = Option<Result<RichLinkResolution, RichLinkError>>;
type FlightReceiver = watch::Receiver<FlightOutcome>;

struct RichLinkResolverState {
    entries: HashMap<String, RichLinkCacheEntry>,
    order: VecDeque<String>,
    in_flight: HashMap<String, FlightReceiver>,
}

impl RichLinkResolverState {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            in_flight: HashMap::new(),
        }
    }

    fn fresh(&mut self, url: &str, now_ms: i64) -> Option<RichLinkCacheEntry> {
        let entry = self.entries.get(url)?;
        if entry.expires_at_ms <= now_ms {
            return None;
        }
        let entry = entry.clone();
        if let Some(position) = self.order.iter().position(|key| key == url) {
            self.order.remove(position);
        }
        self.order.push_back(url.to_owned());
        Some(entry)
    }

    fn retain(&mut self, url: String, entry: RichLinkCacheEntry, capacity: usize) {
        self.entries.insert(url.clone(), entry);
        if let Some(position) = self.order.iter().position(|key| key == &url) {
            self.order.remove(position);
        }
        self.order.push_back(url);
        while self.entries.len() > capacity {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            self.entries.remove(&oldest);
        }
    }
}

struct RichLinkResolverInner {
    fetcher: Arc<dyn RichLinkPageFetcher>,
    options: RichLinkResolverOptions,
    clock: Arc<dyn RichLinkClock>,
    state: Mutex<RichLinkResolverState>,
}

/// Bounded, TTL-cached, in-flight-deduplicated rich-link metadata resolver.
pub struct RichLinkResolver {
    inner: Arc<RichLinkResolverInner>,
}

impl RichLinkResolver {
    /// Builds one resolver over the supplied fetcher and validated limits.
    ///
    /// # Errors
    ///
    /// Returns [`RichLinkError::Configuration`] for invalid limits.
    #[must_use = "a resolver owns the cache and in-flight state"]
    pub fn new(
        fetcher: Arc<dyn RichLinkPageFetcher>,
        options: RichLinkResolverOptions,
    ) -> Result<Self, RichLinkError> {
        Self::with_clock(fetcher, options, Arc::new(SystemRichLinkClock))
    }

    /// Builds one resolver with an explicit clock seam.
    ///
    /// # Errors
    ///
    /// Returns [`RichLinkError::Configuration`] for invalid limits.
    pub fn with_clock(
        fetcher: Arc<dyn RichLinkPageFetcher>,
        options: RichLinkResolverOptions,
        clock: Arc<dyn RichLinkClock>,
    ) -> Result<Self, RichLinkError> {
        let options = options.validate()?;
        Ok(Self {
            inner: Arc::new(RichLinkResolverInner {
                fetcher,
                options,
                clock,
                state: Mutex::new(RichLinkResolverState::new()),
            }),
        })
    }

    /// Builds the production resolver with reference-equivalent limits.
    ///
    /// # Panics
    ///
    /// Panics only if the fixed first-party default limits are invalid.
    #[must_use]
    pub fn with_defaults() -> Self {
        Self::new(
            Arc::new(HttpRichLinkFetcher::with_defaults()),
            RichLinkResolverOptions::default(),
        )
        .expect("fixed rich-link defaults are valid")
    }

    /// Resolves one canonical display title for an assistant-authored URL.
    ///
    /// Fresh cache entries answer without fetching; concurrent misses share
    /// one owner fetch; failures are never cached.
    ///
    /// # Errors
    ///
    /// Returns the first classified [`RichLinkError`] for an invalid or
    /// blocked target, a non-HTML response, an oversized body, a transport
    /// failure, or an elapsed fetch deadline.
    pub async fn resolve(&self, input: &str) -> Result<RichLinkResolution, RichLinkError> {
        let requested_url = canonical_rich_link_url(input)?;
        let cache_key = requested_url.as_str().to_owned();
        let now_ms = self.inner.clock.now_ms();
        let mut state = self.inner.state.lock().await;
        if let Some(entry) = state.fresh(&cache_key, now_ms) {
            return Ok(RichLinkResolution {
                requested_url: cache_key,
                favicon: entry.favicon,
                page_name: entry.page_name,
                expires_at_ms: entry.expires_at_ms,
            });
        }
        if let Some(receiver) = state.in_flight.get(&cache_key).cloned() {
            drop(state);
            return await_flight(receiver).await;
        }
        let (sender, receiver) = watch::channel(None);
        state.in_flight.insert(cache_key.clone(), receiver.clone());
        drop(state);

        let inner = Arc::clone(&self.inner);
        let owner_key = cache_key.clone();
        tokio::spawn(async move {
            let outcome = inner.fetch_resolution(&owner_key).await;
            let mut state = inner.state.lock().await;
            if let Ok(resolution) = &outcome {
                state.retain(
                    owner_key.clone(),
                    RichLinkCacheEntry {
                        favicon: resolution.favicon.clone(),
                        page_name: resolution.page_name.clone(),
                        expires_at_ms: resolution.expires_at_ms,
                    },
                    inner.options.cache_capacity,
                );
            }
            state.in_flight.remove(&owner_key);
            drop(state);
            let _ = sender.send(Some(outcome));
        });

        await_flight(receiver).await
    }
}

impl RichLinkResolverInner {
    async fn fetch_resolution(&self, cache_key: &str) -> Result<RichLinkResolution, RichLinkError> {
        let url = Url::parse(cache_key).map_err(|_| RichLinkError::InvalidUrl)?;
        let page = timeout(self.options.fetch_timeout, self.fetcher.fetch(url))
            .await
            .map_err(|_| RichLinkError::Timeout)??;
        if !is_html_content_type(&page.content_type) {
            return Err(RichLinkError::UnsupportedContentType);
        }
        if page.body.len() > self.options.max_html_bytes {
            return Err(RichLinkError::ResponseTooLarge);
        }
        let parsed = parse_rich_link_html(&String::from_utf8_lossy(&page.body));
        let host = Url::parse(&page.final_url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned));
        let Some(page_name) = parsed
            .page_name()
            .map(str::to_owned)
            .or_else(|| host.and_then(|host| normalize_rich_link_text(&host)))
        else {
            return Err(RichLinkError::UnsupportedContentType);
        };
        let icon_url = Url::parse(&page.final_url).ok().and_then(|base| {
            base.join(parsed.icon.as_deref().unwrap_or("/favicon.ico"))
                .ok()
        });
        let favicon = match icon_url {
            Some(url) => timeout(Duration::from_secs(2), self.fetcher.fetch_icon(url))
                .await
                .unwrap_or_default(),
            None => Vec::new(),
        };
        let favicon = if favicon.len() <= 65_536 {
            favicon
        } else {
            Vec::new()
        };
        let ttl_ms = i64::try_from(self.options.cache_ttl.as_millis()).unwrap_or(i64::MAX);
        Ok(RichLinkResolution {
            favicon,
            requested_url: cache_key.to_owned(),
            page_name,
            expires_at_ms: self.clock.now_ms().saturating_add(ttl_ms),
        })
    }
}

async fn await_flight(mut receiver: FlightReceiver) -> Result<RichLinkResolution, RichLinkError> {
    loop {
        if let Some(outcome) = receiver.borrow().clone() {
            return outcome;
        }
        if receiver.changed().await.is_err() {
            return Err(RichLinkError::Unavailable);
        }
    }
}

/// Test-only cache introspection.
#[cfg(test)]
impl RichLinkResolver {
    async fn retained_entry_count(&self) -> usize {
        self.inner.state.lock().await.entries.len()
    }
}

#[cfg(test)]
mod tests;
