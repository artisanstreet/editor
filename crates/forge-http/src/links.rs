//! Page titles for the links the Editor shows, so a bare URL can read as
//! the page it points to.
//!
//! One short, bounded fetch per URL: `http`/`https` only, public addresses
//! only, five seconds, five redirects, the first 512 KiB of an HTML page.
//! Links come from engine output, so a link must not make Forge reach into
//! the machine or network it runs on. The title is the page's
//! `og:title`, else its `<title>`, entity-decoded with whitespace collapsed.
//! Answers are cached in memory: a day for a title, ten minutes for a page
//! without one or a fetch that failed.

use anyhow::{Context as _, Result, bail};
use reqwest::Url;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const FETCH_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_REDIRECTS: usize = 5;
const MAX_BODY_BYTES: usize = 512 * 1024;
const FOUND_TTL: Duration = Duration::from_hours(24);
const MISSING_TTL: Duration = Duration::from_mins(10);
const MAX_CACHED: usize = 1024;
/// Longer titles are cut; the Editor caps what it shows further.
const MAX_TITLE_CHARS: usize = 300;

pub(crate) struct LinkTitles {
    http: reqwest::Client,
    /// Off except in tests, which serve pages from loopback.
    private: Arc<AtomicBool>,
    cache: Mutex<HashMap<String, Cached>>,
}

struct Cached {
    title: Option<String>,
    expires: Instant,
}

impl LinkTitles {
    pub(crate) fn new() -> Result<Self> {
        let private = Arc::new(AtomicBool::new(false));
        let redirects = Arc::clone(&private);
        let http = reqwest::Client::builder()
            .timeout(FETCH_TIMEOUT)
            .dns_resolver(Arc::new(PublicResolver(Arc::clone(&private))))
            .redirect(reqwest::redirect::Policy::custom(move |attempt| {
                if attempt.previous().len() >= MAX_REDIRECTS {
                    attempt.error("too many redirects")
                } else if !redirects.load(Ordering::Relaxed) && !literal_is_public(attempt.url()) {
                    attempt.error("redirected to a private address")
                } else {
                    attempt.follow()
                }
            }))
            .user_agent(concat!("Artisan-Forge/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("build the link title client")?;
        Ok(Self {
            http,
            private,
            cache: Mutex::new(HashMap::new()),
        })
    }

    /// Lets links reach loopback and private addresses, for tests that
    /// serve pages locally.
    pub(crate) fn reach_private(&self) {
        self.private.store(true, Ordering::Relaxed);
    }

    /// The title of the page at `url`, from cache when it is fresh.
    pub(crate) async fn title(&self, url: Url) -> Option<String> {
        let key = url.as_str().to_owned();
        let now = Instant::now();
        if let Some(cached) = self.lock().get(&key).filter(|cached| cached.expires > now) {
            return cached.title.clone();
        }
        let reachable = self.private.load(Ordering::Relaxed) || literal_is_public(&url);
        let fetched = if reachable {
            fetch(&self.http, url).await
        } else {
            Err(anyhow::anyhow!("the link points at a private address"))
        };
        let title = match fetched {
            Ok(title) => title,
            Err(error) => {
                tracing::debug!(url = %key, "no link title: {error:#}");
                None
            }
        };
        let ttl = if title.is_some() {
            FOUND_TTL
        } else {
            MISSING_TTL
        };
        let mut cache = self.lock();
        if cache.len() >= MAX_CACHED {
            let now = Instant::now();
            cache.retain(|_, cached| cached.expires > now);
            if cache.len() >= MAX_CACHED
                && let Some(soonest) = cache
                    .iter()
                    .min_by_key(|(_, cached)| cached.expires)
                    .map(|(key, _)| key.clone())
            {
                cache.remove(&soonest);
            }
        }
        cache.insert(
            key,
            Cached {
                title: title.clone(),
                expires: Instant::now() + ttl,
            },
        );
        title
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Cached>> {
        self.cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// An absolute `http`/`https` URL, without its fragment (the page is the
/// same either way).
pub(crate) fn link_url(input: &str) -> Result<Url> {
    let mut url = Url::parse(input.trim()).context("not a URL")?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        bail!("only http and https links have titles");
    }
    url.set_fragment(None);
    Ok(url)
}

/// Resolves names to their public addresses only, so neither a link nor a
/// redirect nor a rebinding DNS answer reaches a private one.
struct PublicResolver(Arc<AtomicBool>);

impl Resolve for PublicResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let private = self.0.load(Ordering::Relaxed);
        Box::pin(async move {
            let found: Vec<SocketAddr> = tokio::net::lookup_host((name.as_str(), 0))
                .await?
                .filter(|address| private || is_public(address.ip()))
                .collect();
            if found.is_empty() {
                return Err(format!("{} has no public address", name.as_str()).into());
            }
            let addrs: Addrs = Box::new(found.into_iter());
            Ok(addrs)
        })
    }
}

/// False when `url` names a private IP address directly (those skip the
/// resolver); names are checked when they resolve.
fn literal_is_public(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    host.trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<IpAddr>()
        .map_or(true, is_public)
}

fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_public_v4(ip),
        IpAddr::V6(ip) => match ip.to_ipv4_mapped() {
            Some(mapped) => is_public_v4(mapped),
            None => is_public_v6(ip),
        },
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_multicast()
        || a == 0
        // Shared address space (carrier-grade NAT) and benchmarking.
        || (a == 100 && (64..128).contains(&b))
        || (a == 198 && (18..20).contains(&b))
        || a >= 240)
}

fn is_public_v6(ip: Ipv6Addr) -> bool {
    let first = ip.segments()[0];
    !(ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        // Unique local fc00::/7 and link-local fe80::/10.
        || (first & 0xfe00) == 0xfc00
        || (first & 0xffc0) == 0xfe80)
}

async fn fetch(http: &reqwest::Client, url: Url) -> Result<Option<String>> {
    let mut response = http
        .get(url)
        .header(ACCEPT, "text/html,application/xhtml+xml")
        .send()
        .await
        .context("request the page")?
        .error_for_status()
        .context("request the page")?;
    let kind = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase()
        });
    if let Some(kind) = kind
        && !matches!(kind.as_str(), "text/html" | "application/xhtml+xml")
    {
        bail!("not an HTML page but {kind}");
    }
    let mut body = Vec::new();
    while body.len() < MAX_BODY_BYTES
        && let Some(chunk) = response.chunk().await.context("read the page")?
    {
        body.extend_from_slice(&chunk);
    }
    body.truncate(MAX_BODY_BYTES);
    Ok(page_title(&String::from_utf8_lossy(&body)))
}

/// `og:title`, else `<title>`. Scans tags up to `<body>`, tolerating the
/// unclosed and malformed markup real pages carry.
fn page_title(html: &str) -> Option<String> {
    // ASCII lowercasing keeps every byte offset, so offsets into `lower`
    // slice `html` too.
    let lower = html.to_ascii_lowercase();
    let mut title = None;
    let mut at = 0;
    while let Some(offset) = lower[at..].find('<') {
        let start = at + offset + 1;
        let rest = &lower[start..];
        if rest.starts_with("!--") {
            at = rest.find("-->").map_or(lower.len(), |end| start + end + 3);
            continue;
        }
        let name_len = rest
            .find(|c: char| c.is_ascii_whitespace() || c == '>' || c == '/')
            .unwrap_or(rest.len());
        let name = &rest[..name_len];
        let Some(end) = tag_end(&lower, start + name_len) else {
            break;
        };
        match name {
            "meta" => {
                let attributes = attributes(&html[start + name_len..end]);
                let key = attributes
                    .property
                    .as_deref()
                    .or(attributes.name.as_deref());
                if key.is_some_and(|key| key.trim().eq_ignore_ascii_case("og:title"))
                    && let Some(og) = attributes.content.as_deref().and_then(clean)
                {
                    return Some(og);
                }
            }
            "title" | "script" | "style" => {
                let text = end + 1;
                let close = lower[text..]
                    .find(&format!("</{name}"))
                    .map_or(lower.len(), |close| text + close);
                if name == "title" && title.is_none() {
                    title = clean(&decode_entities(&html[text..close]));
                }
                at = close;
                continue;
            }
            "body" => break,
            _ => {}
        }
        at = end + 1;
    }
    title
}

/// The `>` closing a tag, skipping quoted attribute values.
fn tag_end(lower: &str, from: usize) -> Option<usize> {
    let mut quote = None;
    for (index, byte) in lower.bytes().enumerate().skip(from) {
        match (quote, byte) {
            (Some(open), _) if byte == open => quote = None,
            (None, b'"' | b'\'') => quote = Some(byte),
            (None, b'>') => return Some(index),
            _ => {}
        }
    }
    None
}

#[derive(Default)]
struct Attributes {
    property: Option<String>,
    name: Option<String>,
    content: Option<String>,
}

fn attributes(source: &str) -> Attributes {
    let bytes = source.as_bytes();
    let mut found = Attributes::default();
    let mut index = 0;
    let skip_space = |index: &mut usize| {
        while bytes.get(*index).is_some_and(u8::is_ascii_whitespace) {
            *index += 1;
        }
    };
    while index < bytes.len() {
        skip_space(&mut index);
        let name_start = index;
        while bytes
            .get(index)
            .is_some_and(|byte| !byte.is_ascii_whitespace() && !matches!(byte, b'=' | b'/'))
        {
            index += 1;
        }
        if name_start == index {
            index += 1;
            continue;
        }
        let name = source[name_start..index].to_ascii_lowercase();
        skip_space(&mut index);
        let mut value = String::new();
        if bytes.get(index) == Some(&b'=') {
            index += 1;
            skip_space(&mut index);
            let quote = bytes
                .get(index)
                .copied()
                .filter(|byte| matches!(byte, b'"' | b'\''));
            if quote.is_some() {
                index += 1;
            }
            let value_start = index;
            while bytes.get(index).is_some_and(|byte| match quote {
                Some(quote) => *byte != quote,
                None => !byte.is_ascii_whitespace(),
            }) {
                index += 1;
            }
            value = decode_entities(&source[value_start..index]);
            if quote.is_some() {
                index += 1;
            }
        }
        match name.as_str() {
            "property" => found.property = Some(value),
            "name" => found.name = Some(value),
            "content" => found.content = Some(value),
            _ => {}
        }
    }
    found
}

/// Collapses whitespace and trims; `None` when nothing is left.
fn clean(text: &str) -> Option<String> {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (!collapsed.is_empty()).then(|| collapsed.chars().take(MAX_TITLE_CHARS).collect())
}

fn decode_entities(text: &str) -> String {
    let mut decoded = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        decoded.push_str(&rest[..start]);
        let after = &rest[start..];
        if let Some((end, character)) = after
            .find(';')
            .filter(|end| *end <= 32)
            .and_then(|end| Some((end, entity(&after[1..end])?)))
        {
            decoded.push(character);
            rest = &after[end + 1..];
        } else {
            decoded.push('&');
            rest = &after[1..];
        }
    }
    decoded.push_str(rest);
    decoded
}

fn entity(name: &str) -> Option<char> {
    if let Some(number) = name.strip_prefix('#') {
        let code = match number.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => number.parse().ok()?,
        };
        return char::from_u32(code);
    }
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{a0}',
        "mdash" => '\u{2014}',
        "ndash" => '\u{2013}',
        "hellip" => '\u{2026}',
        "middot" => '\u{b7}',
        "bull" => '\u{2022}',
        "copy" => '\u{a9}',
        "reg" => '\u{ae}',
        "trade" => '\u{2122}',
        "lsquo" => '\u{2018}',
        "rsquo" => '\u{2019}',
        "ldquo" => '\u{201c}',
        "rdquo" => '\u{201d}',
        _ => return None,
    })
}
