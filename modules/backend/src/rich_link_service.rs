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
    net::IpAddr,
    pin::Pin,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use reqwest::{Url, header::CONTENT_TYPE};
use tokio::{
    sync::{Mutex, watch},
    time::timeout,
};

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

fn is_public_rich_link_host(host: &str) -> bool {
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

fn is_public_address(address: IpAddr) -> bool {
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

/// Normalized text-only metadata extracted from one HTML document.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ParsedRichLinkHtml {
    /// Document `<title>` text, normalized and bounded.
    pub title: Option<String>,
    /// First `og:title` metadata value.
    pub open_graph: Option<String>,
    /// First `twitter:title` metadata value.
    pub twitter: Option<String>,
}

impl ParsedRichLinkHtml {
    /// Returns the reference-ordered page name, if any candidate exists.
    #[must_use]
    pub fn page_name(&self) -> Option<&str> {
        self.open_graph
            .as_deref()
            .or(self.twitter.as_deref())
            .or(self.title.as_deref())
    }
}

/// Parses metadata with the reference precedence and bounds.
///
/// The scanner never allocates from document structure and tolerates the
/// malformed, unclosed tags real pages carry. Text fields are entity-decoded,
/// whitespace-collapsed, trimmed, and bounded to
/// [`RICH_LINK_PAGE_NAME_MAX_CHARS`] scalar values.
#[must_use]
pub fn parse_rich_link_html(html: &str) -> ParsedRichLinkHtml {
    let lower = html.to_ascii_lowercase();
    let mut parsed = ParsedRichLinkHtml::default();
    let mut cursor = 0_usize;
    while let Some(offset) = lower[cursor..].find('<') {
        let tag_start = cursor + offset;
        let rest_lower = &lower[tag_start + 1..];
        if rest_lower.starts_with("!--") {
            match rest_lower.find("-->") {
                Some(end) => cursor = tag_start + 1 + end + 3,
                None => break,
            }
            continue;
        }
        if rest_lower.starts_with('!') || rest_lower.starts_with('/') || rest_lower.starts_with('?')
        {
            match find_tag_end(&lower, tag_start + 1) {
                Some(end) => cursor = end + 1,
                None => break,
            }
            continue;
        }
        let name_end = rest_lower
            .find(|character: char| {
                character.is_ascii_whitespace() || character == '>' || character == '/'
            })
            .unwrap_or(rest_lower.len());
        let name = &rest_lower[..name_end];
        let Some(tag_end) = find_tag_end(&lower, tag_start + 1 + name_end) else {
            break;
        };
        match name {
            "title" if parsed.title.is_none() => {
                let text_start = tag_end + 1;
                let (text, next) = match lower[text_start..].find("</title") {
                    Some(close) => {
                        let text_end = text_start + close;
                        let after = lower[text_end..]
                            .find('>')
                            .map_or(lower.len(), |offset| text_end + offset + 1);
                        (&html[text_start..text_end], after)
                    }
                    None => (&html[text_start..], lower.len()),
                };
                parsed.title = normalize_rich_link_text(&decode_rich_link_entities(text));
                cursor = next;
                continue;
            }
            "meta" => {
                let attributes = &html[tag_start + 1 + name_end..tag_end];
                let meta = parse_meta_attributes(attributes);
                apply_meta(&mut parsed, &meta);
            }
            _ => {}
        }
        cursor = tag_end + 1;
    }
    parsed
}

fn apply_meta(parsed: &mut ParsedRichLinkHtml, meta: &MetaAttributes) {
    let Some(content) = meta.content.as_deref() else {
        return;
    };
    let key = meta
        .property
        .as_deref()
        .or(meta.name.as_deref())
        .map(|value| value.trim().to_ascii_lowercase());
    match key.as_deref() {
        Some("og:title") if parsed.open_graph.is_none() => {
            parsed.open_graph = normalize_rich_link_text(content);
        }
        Some("twitter:title") if parsed.twitter.is_none() => {
            parsed.twitter = normalize_rich_link_text(content);
        }
        _ => {}
    }
}

#[derive(Default)]
struct MetaAttributes {
    property: Option<String>,
    name: Option<String>,
    content: Option<String>,
}

fn parse_meta_attributes(attributes: &str) -> MetaAttributes {
    let bytes = attributes.as_bytes();
    let mut meta = MetaAttributes::default();
    let mut index = 0_usize;
    while index < bytes.len() {
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        let name_start = index;
        while index < bytes.len()
            && !bytes[index].is_ascii_whitespace()
            && bytes[index] != b'='
            && bytes[index] != b'/'
        {
            index += 1;
        }
        if name_start == index {
            index += 1;
            continue;
        }
        let attribute_name = attributes[name_start..index].to_ascii_lowercase();
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        let value = if bytes.get(index) == Some(&b'=') {
            index += 1;
            while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                index += 1;
            }
            read_attribute_value(attributes, &mut index)
        } else {
            String::new()
        };
        match attribute_name.as_str() {
            "property" => meta.property = Some(value),
            "name" => meta.name = Some(value),
            "content" => meta.content = Some(value),
            _ => {}
        }
    }
    meta
}

fn read_attribute_value(attributes: &str, index: &mut usize) -> String {
    let bytes = attributes.as_bytes();
    if bytes
        .get(*index)
        .is_some_and(|byte| *byte == b'"' || *byte == b'\'')
    {
        let quote = bytes[*index];
        *index += 1;
        let value_start = *index;
        while *index < bytes.len() && bytes[*index] != quote {
            *index += 1;
        }
        let value = decode_rich_link_entities(&attributes[value_start..*index]);
        if *index < bytes.len() {
            *index += 1;
        }
        return value;
    }
    let value_start = *index;
    while *index < bytes.len() && !bytes[*index].is_ascii_whitespace() {
        *index += 1;
    }
    decode_rich_link_entities(&attributes[value_start..*index])
}

fn find_tag_end(lower: &str, from: usize) -> Option<usize> {
    let bytes = lower.as_bytes();
    let mut quote: Option<u8> = None;
    let mut index = from;
    while index < bytes.len() {
        let byte = bytes[index];
        match quote {
            Some(active) => {
                if byte == active {
                    quote = None;
                }
            }
            None => {
                if byte == b'"' || byte == b'\'' {
                    quote = Some(byte);
                } else if byte == b'>' {
                    return Some(index);
                }
            }
        }
        index += 1;
    }
    None
}

/// Collapses whitespace, trims, and bounds one metadata text field.
///
/// Returns `None` when the normalized field is empty.
#[must_use]
pub fn normalize_rich_link_text(value: &str) -> Option<String> {
    let collapsed = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return None;
    }
    Some(
        collapsed
            .chars()
            .take(RICH_LINK_PAGE_NAME_MAX_CHARS)
            .collect(),
    )
}

fn decode_rich_link_entities(value: &str) -> String {
    if !value.contains('&') {
        return value.to_owned();
    }
    let mut decoded = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find('&') {
        decoded.push_str(&rest[..start]);
        let after = &rest[start..];
        let Some(end) = after.find(';') else {
            decoded.push_str(after);
            return decoded;
        };
        if end > 32 {
            decoded.push_str(&after[..=end]);
            rest = &after[end + 1..];
            continue;
        }
        if let Some(character) = decode_entity(&after[1..end]) {
            decoded.push(character);
        } else {
            decoded.push_str(&after[..=end]);
        }
        rest = &after[end + 1..];
    }
    decoded.push_str(rest);
    decoded
}

fn decode_entity(entity: &str) -> Option<char> {
    if let Some(number) = entity.strip_prefix('#') {
        let value = if let Some(hex) = number
            .strip_prefix('x')
            .or_else(|| number.strip_prefix('X'))
        {
            u32::from_str_radix(hex, 16).ok()?
        } else {
            number.parse::<u32>().ok()?
        };
        return char::from_u32(value);
    }
    let character = match entity {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{a0}',
        "mdash" => '\u{2014}',
        "ndash" => '\u{2013}',
        "hellip" => '\u{2026}',
        "copy" => '\u{a9}',
        "reg" => '\u{ae}',
        "trade" => '\u{2122}',
        "lsquo" => '\u{2018}',
        "rsquo" => '\u{2019}',
        "ldquo" => '\u{201c}',
        "rdquo" => '\u{201d}',
        "middot" => '\u{b7}',
        "bull" => '\u{2022}',
        _ => return None,
    };
    Some(character)
}

#[derive(Clone)]
struct RichLinkCacheEntry {
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
        let ttl_ms = i64::try_from(self.options.cache_ttl.as_millis()).unwrap_or(i64::MAX);
        Ok(RichLinkResolution {
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
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicI64, AtomicUsize, Ordering},
    };

    use super::*;

    struct ManualClock(AtomicI64);

    impl ManualClock {
        fn new(now_ms: i64) -> Self {
            Self(AtomicI64::new(now_ms))
        }

        fn advance(&self, delta_ms: i64) {
            self.0.fetch_add(delta_ms, Ordering::SeqCst);
        }
    }

    impl RichLinkClock for ManualClock {
        fn now_ms(&self) -> i64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    struct ScriptedFetcher {
        calls: AtomicUsize,
        delay: Duration,
        page: Result<RichLinkPage, RichLinkError>,
    }

    impl ScriptedFetcher {
        fn new(page: Result<RichLinkPage, RichLinkError>, delay: Duration) -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicUsize::new(0),
                delay,
                page,
            })
        }

        fn page(html: &str) -> Arc<Self> {
            Self::new(
                Ok(RichLinkPage {
                    final_url: "https://example.com/final".to_owned(),
                    content_type: "text/html".to_owned(),
                    body: html.as_bytes().to_vec(),
                }),
                Duration::ZERO,
            )
        }

        fn failure(error: RichLinkError) -> Arc<Self> {
            Self::new(Err(error), Duration::ZERO)
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl RichLinkPageFetcher for ScriptedFetcher {
        fn fetch(&self, _url: Url) -> RichLinkFetchFuture {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let page = self.page.clone();
            let delay = self.delay;
            Box::pin(async move {
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
                page
            })
        }
    }

    fn resolver(fetcher: Arc<ScriptedFetcher>) -> RichLinkResolver {
        RichLinkResolver::new(fetcher, RichLinkResolverOptions::default())
            .expect("default options are valid")
    }

    #[test]
    fn parser_prefers_open_graph_then_twitter_then_document_title() {
        let html = r#"<!doctype html>
            <html><head>
            <meta content="Twitter &amp; Co" name="twitter:title">
            <TITLE>  Document
                Title  </TITLE>
            <meta property="og:title" content="Open Graph Title">
            </head></html>"#;
        let parsed = parse_rich_link_html(html);
        assert_eq!(parsed.title.as_deref(), Some("Document Title"));
        assert_eq!(parsed.twitter.as_deref(), Some("Twitter & Co"));
        assert_eq!(parsed.open_graph.as_deref(), Some("Open Graph Title"));
        assert_eq!(parsed.page_name(), Some("Open Graph Title"));

        let title_only =
            parse_rich_link_html("<html><head><title>Only Title</title></head></html>");
        assert_eq!(title_only.page_name(), Some("Only Title"));

        let twitter_only = parse_rich_link_html(
            "<meta property=\"og:title\"><meta name=\"twitter:title\" content=\"Twitter First\">",
        );
        assert_eq!(twitter_only.page_name(), Some("Twitter First"));

        let name_og = parse_rich_link_html("<meta name=\"og:title\" content=\"Named OG\">");
        assert_eq!(name_og.page_name(), Some("Named OG"));

        let first_wins = parse_rich_link_html(
            "<meta property=\"og:title\" content=\"First\"><meta property=\"og:title\" content=\"Second\">",
        );
        assert_eq!(first_wins.page_name(), Some("First"));
    }

    #[test]
    fn parser_ignores_degenerate_bodies_and_comments() {
        assert_eq!(parse_rich_link_html(""), ParsedRichLinkHtml::default());
        assert_eq!(
            parse_rich_link_html("just plain text, no markup"),
            ParsedRichLinkHtml::default()
        );
        assert_eq!(
            parse_rich_link_html("<html><body>no head tags</body></html>"),
            ParsedRichLinkHtml::default()
        );
        assert_eq!(
            parse_rich_link_html("<title>   </title>"),
            ParsedRichLinkHtml::default()
        );
        assert_eq!(
            parse_rich_link_html("<!-- <title>Commented</title> -->"),
            ParsedRichLinkHtml::default()
        );
        assert_eq!(
            parse_rich_link_html("<meta property=\"og:title\" content=\"\">"),
            ParsedRichLinkHtml::default()
        );
        assert_eq!(
            parse_rich_link_html("<meta property=\"og:title\" content=\"Unquoted Title\">")
                .page_name(),
            Some("Unquoted Title")
        );
        assert_eq!(
            parse_rich_link_html("<title>Unclosed Title").page_name(),
            Some("Unclosed Title")
        );
    }

    #[test]
    fn parser_bounds_and_entity_decodes_metadata_text() {
        let long_title = "x".repeat(RICH_LINK_PAGE_NAME_MAX_CHARS + 100);
        let parsed = parse_rich_link_html(&format!("<title>{long_title}</title>"));
        let page_name = parsed.page_name().expect("bounded title survives");
        assert_eq!(page_name.chars().count(), RICH_LINK_PAGE_NAME_MAX_CHARS);

        let decoded = parse_rich_link_html(
            "<title>&#72;&#x65;llo &amp; &quot;world&quot; &hellip; &unknown;</title>",
        );
        assert_eq!(decoded.page_name(), Some("Hello & \"world\" … &unknown;"));
    }

    #[tokio::test]
    async fn resolve_uses_host_fallback_for_titleless_html() {
        let fetcher = ScriptedFetcher::page("<html><body>No title here</body></html>");
        let resolver = resolver(fetcher);
        let resolution = resolver
            .resolve("https://example.com/docs#section")
            .await
            .expect("host fallback resolves");
        assert_eq!(resolution.requested_url, "https://example.com/docs");
        assert_eq!(resolution.page_name, "example.com");
        assert!(resolution.expires_at_ms > 0);
    }

    #[tokio::test]
    async fn resolve_returns_resolved_title_and_reuses_cache() {
        let fetcher = ScriptedFetcher::page("<title>Resolved Page</title>");
        let resolver = resolver(Arc::clone(&fetcher));
        let first = resolver
            .resolve("https://example.com/page")
            .await
            .expect("resolves");
        assert_eq!(first.page_name, "Resolved Page");
        let second = resolver
            .resolve("https://example.com/page")
            .await
            .expect("cached resolves");
        assert_eq!(second.expires_at_ms, first.expires_at_ms);
        assert_eq!(fetcher.calls(), 1);
    }

    #[tokio::test]
    async fn resolve_rejects_non_html_and_oversized_bodies() {
        let json = ScriptedFetcher::new(
            Ok(RichLinkPage {
                final_url: "https://example.com/data".to_owned(),
                content_type: "application/json".to_owned(),
                body: b"{\"title\":\"Not HTML\"}".to_vec(),
            }),
            Duration::ZERO,
        );
        assert_eq!(
            resolver(json).resolve("https://example.com/data").await,
            Err(RichLinkError::UnsupportedContentType)
        );

        let oversized = ScriptedFetcher::page(&"x".repeat(128));
        let resolver = RichLinkResolver::new(
            oversized,
            RichLinkResolverOptions {
                cache_ttl: Duration::from_secs(60),
                cache_capacity: 4,
                max_html_bytes: 32,
                fetch_timeout: Duration::from_secs(1),
            },
        )
        .expect("valid bounded options");
        assert_eq!(
            resolver.resolve("https://example.com/large").await,
            Err(RichLinkError::ResponseTooLarge)
        );
    }

    #[tokio::test]
    async fn cache_ttl_expires_and_refetches() {
        let fetcher = ScriptedFetcher::page("<title>TTL Page</title>");
        let clock = Arc::new(ManualClock::new(1_000));
        let resolver = RichLinkResolver::with_clock(
            Arc::clone(&fetcher) as Arc<dyn RichLinkPageFetcher>,
            RichLinkResolverOptions {
                cache_ttl: Duration::from_millis(1_000),
                cache_capacity: 4,
                max_html_bytes: RICH_LINK_DEFAULT_MAX_HTML_BYTES,
                fetch_timeout: Duration::from_secs(1),
            },
            Arc::clone(&clock) as Arc<dyn RichLinkClock>,
        )
        .expect("valid options");

        let first = resolver
            .resolve("https://example.com/ttl")
            .await
            .expect("first resolves");
        assert_eq!(first.expires_at_ms, 2_000);
        resolver
            .resolve("https://example.com/ttl")
            .await
            .expect("fresh cache hit");
        assert_eq!(fetcher.calls(), 1);

        clock.advance(1_000);
        let expired = resolver
            .resolve("https://example.com/ttl")
            .await
            .expect("expired entry refetches");
        assert_eq!(expired.expires_at_ms, 3_000);
        assert_eq!(fetcher.calls(), 2);
    }

    #[tokio::test]
    async fn cache_is_bounded_and_refreshes_recency() {
        let fetcher = ScriptedFetcher::page("<title>Bounded</title>");
        let resolver = RichLinkResolver::new(
            Arc::clone(&fetcher) as Arc<dyn RichLinkPageFetcher>,
            RichLinkResolverOptions {
                cache_ttl: Duration::from_secs(60),
                cache_capacity: 2,
                max_html_bytes: RICH_LINK_DEFAULT_MAX_HTML_BYTES,
                fetch_timeout: Duration::from_secs(1),
            },
        )
        .expect("valid options");

        resolver.resolve("https://example.com/a").await.unwrap();
        resolver.resolve("https://example.com/b").await.unwrap();
        // Refreshing `a` makes `b` the least recently used entry.
        resolver.resolve("https://example.com/a").await.unwrap();
        // Inserting `c` at capacity 2 evicts the least recently used `b`.
        resolver.resolve("https://example.com/c").await.unwrap();
        assert_eq!(resolver.retained_entry_count().await, 2);
        assert_eq!(fetcher.calls(), 3);

        // Both retained entries still answer from cache.
        resolver.resolve("https://example.com/c").await.unwrap();
        resolver.resolve("https://example.com/a").await.unwrap();
        assert_eq!(fetcher.calls(), 3);

        // `b` was evicted and must fetch again; that insert evicts `c`.
        resolver.resolve("https://example.com/b").await.unwrap();
        assert_eq!(resolver.retained_entry_count().await, 2);
        assert_eq!(fetcher.calls(), 4);

        // `c` was the least recently used entry and must fetch again.
        resolver.resolve("https://example.com/c").await.unwrap();
        assert_eq!(fetcher.calls(), 5);
        assert_eq!(resolver.retained_entry_count().await, 2);
    }

    #[tokio::test]
    async fn concurrent_resolves_share_one_fetch() {
        let fetcher = Arc::new(ScriptedFetcher {
            calls: AtomicUsize::new(0),
            delay: Duration::from_millis(50),
            page: Ok(RichLinkPage {
                final_url: "https://example.com/shared".to_owned(),
                content_type: "text/html".to_owned(),
                body: b"<title>Shared Flight</title>".to_vec(),
            }),
        });
        let resolver = resolver(Arc::clone(&fetcher));
        let (first, second) = tokio::join!(
            resolver.resolve("https://example.com/shared"),
            resolver.resolve("https://example.com/shared")
        );
        let first = first.expect("first resolves");
        let second = second.expect("follower resolves");
        assert_eq!(first, second);
        assert_eq!(first.page_name, "Shared Flight");
        assert_eq!(fetcher.calls(), 1);
    }

    #[tokio::test]
    async fn failures_are_not_cached_and_timeout_is_bounded() {
        let failing = ScriptedFetcher::failure(RichLinkError::Transport);
        let resolver = resolver(Arc::clone(&failing));
        assert_eq!(
            resolver.resolve("https://example.com/failure").await,
            Err(RichLinkError::Transport)
        );
        assert_eq!(
            resolver.resolve("https://example.com/failure").await,
            Err(RichLinkError::Transport)
        );
        assert_eq!(failing.calls(), 2);
        assert_eq!(resolver.retained_entry_count().await, 0);

        let slow = Arc::new(ScriptedFetcher {
            calls: AtomicUsize::new(0),
            delay: Duration::from_secs(10),
            page: Ok(RichLinkPage {
                final_url: "https://example.com/slow".to_owned(),
                content_type: "text/html".to_owned(),
                body: b"<title>Slow</title>".to_vec(),
            }),
        });
        let resolver = RichLinkResolver::new(
            Arc::clone(&slow) as Arc<dyn RichLinkPageFetcher>,
            RichLinkResolverOptions {
                cache_ttl: Duration::from_secs(60),
                cache_capacity: 4,
                max_html_bytes: RICH_LINK_DEFAULT_MAX_HTML_BYTES,
                fetch_timeout: Duration::from_millis(50),
            },
        )
        .expect("valid options");
        let started = std::time::Instant::now();
        assert_eq!(
            resolver.resolve("https://example.com/slow").await,
            Err(RichLinkError::Timeout)
        );
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn canonical_url_policy_rejects_non_http_and_non_public_targets() {
        assert_eq!(
            canonical_rich_link_url("https://example.com/docs?q=1#frag")
                .expect("public URL is accepted")
                .as_str(),
            "https://example.com/docs?q=1"
        );
        assert!(canonical_rich_link_url("http://8.8.8.8/").is_ok());
        assert_eq!(
            canonical_rich_link_url("ftp://example.com/"),
            Err(RichLinkError::InvalidUrl)
        );
        assert_eq!(
            canonical_rich_link_url("example.com/path"),
            Err(RichLinkError::InvalidUrl)
        );
        assert_eq!(
            canonical_rich_link_url("https://user:secret@example.com/"),
            Err(RichLinkError::InvalidUrl)
        );
        for blocked in [
            "http://localhost/",
            "http://docs.localhost/",
            "http://127.0.0.1/",
            "http://10.1.2.3/",
            "http://192.168.0.1/",
            "http://172.16.4.5/",
            "http://169.254.10.10/",
            "http://0.0.0.0/",
            "http://100.64.0.1/",
            "http://[::1]/",
            "http://[fd00::1]/",
        ] {
            assert_eq!(
                canonical_rich_link_url(blocked),
                Err(RichLinkError::BlockedAddress),
                "{blocked} must be blocked"
            );
        }
    }
}
