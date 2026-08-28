//! Pure validation and storage-boundary policy for the Forge endpoint.
//!
//! This is the native counterpart of
//! `modules/frontend/src/lib/runtime/forge-endpoint.ts`. It deliberately
//! stops at values and intents: callers own browser or desktop storage,
//! network requests, URL objects supplied by a host runtime, and any
//! asynchronous execution.

use std::net::Ipv6Addr;

/// The exact storage key used by the browser Forge endpoint service.
pub const FORGE_ENDPOINT_STORAGE_KEY: &str = "artisan.forge-endpoint";

/// The inclusive UTF-16 code-unit bound used for endpoint candidates.
pub const MAX_FORGE_ENDPOINT_CODE_UNITS: usize = 256;

const HTTP_DEFAULT_PORT: u32 = 80;

/// A storage read observed by the endpoint policy.
///
/// The adapter supplies an owned value so a read result can outlive the
/// storage operation. A `Value` is not trusted merely because it was read;
/// [`resolve_forge_endpoint`] validates it again.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ForgeEndpointStorageReadResult {
    /// The endpoint key was not present.
    Missing,
    /// A value was returned for the endpoint key.
    Value(String),
    /// Reading the endpoint key failed.
    ReadFailure,
}

/// The result observed after a best-effort endpoint storage write.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ForgeEndpointStorageWriteResult {
    /// The write completed successfully.
    Succeeded,
    /// The write failed; the in-memory adoption remains valid.
    Failed,
}

/// A write request for the endpoint storage adapter.
///
/// The value is already normalized by [`adopt_forge_endpoint`]. Executing
/// this request is outside this module.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ForgeEndpointStorageIntent {
    /// The exact key the adapter should write.
    pub key: &'static str,
    /// The normalized loopback origin to persist.
    pub value: String,
}

impl ForgeEndpointStorageIntent {
    /// Creates a write intent for one normalized endpoint origin.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            key: FORGE_ENDPOINT_STORAGE_KEY,
            value: value.into(),
        }
    }
}

/// The pure result of a valid endpoint adoption after its write was observed.
///
/// Both write outcomes retain the endpoint. The TypeScript service treats a
/// storage write error as best effort, so a failed write cannot undo a valid
/// in-memory adoption.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ForgeEndpointAdoptionResult {
    /// The normalized loopback origin adopted by the caller.
    pub endpoint: String,
    /// The storage outcome observed while persisting the adoption.
    pub storage: ForgeEndpointStorageWriteOutcome,
}

/// The normalized outcome of a best-effort endpoint storage write.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ForgeEndpointStorageWriteOutcome {
    /// The endpoint was persisted.
    Persisted,
    /// The write failed, but the endpoint adoption was retained.
    FailureAbsorbed,
}

impl ForgeEndpointStorageWriteOutcome {
    /// Returns whether the storage failure was intentionally absorbed.
    #[must_use]
    pub const fn is_failure_absorbed(self) -> bool {
        matches!(self, Self::FailureAbsorbed)
    }
}

/// Whether a page protocol may carry a Forge endpoint.
///
/// This comparison is intentionally case-sensitive, matching the reached
/// TypeScript predicate. Browser `location.protocol` is normally lowercase;
/// callers that supply another protocol spelling receive the same native
/// decision as the source contract.
#[must_use]
pub fn is_endpoint_bearing_page(page_protocol: &str) -> bool {
    page_protocol != "http:" && page_protocol != "https:"
}

/// Alias with the vocabulary used by the source policy.
#[must_use]
pub fn endpoint_bearing_page(page_protocol: &str) -> bool {
    is_endpoint_bearing_page(page_protocol)
}

/// Decodes a candidate into a normalized, safe loopback HTTP origin.
///
/// Candidates are limited by UTF-16 code units, as JavaScript's
/// `String.length` is. Only HTTP, the two loopback hosts accepted by the
/// source policy, no credentials, and an explicit non-default port are
/// admitted. The returned origin excludes path, query, and fragment.
#[must_use]
pub fn decode_loopback_forge_endpoint(candidate: &str) -> Option<String> {
    if !candidate_length_is_valid(candidate) {
        return None;
    }

    let candidate = candidate.trim_matches(|character: char| character <= '\u{20}');
    let scheme_end = candidate.find(':')?;
    if !candidate[..scheme_end].eq_ignore_ascii_case("http") {
        return None;
    }

    let after_scheme = candidate.get(scheme_end + 1..)?;
    if !after_scheme.starts_with("//") {
        return None;
    }

    let authority_source = after_scheme.get(2..)?;
    let authority_end = authority_source
        .find(['/', '?', '#'])
        .unwrap_or(authority_source.len());
    let authority = &authority_source[..authority_end];
    let (host, port) = parse_authority(authority)?;

    Some(format!("http://{host}:{port}"))
}

/// Builds a storage write intent for a valid endpoint adoption.
///
/// `None` means either that the page is an ordinary HTTP(S) page or that the
/// candidate is not a valid loopback endpoint. No storage request is emitted
/// in either case.
#[must_use]
pub fn adopt_forge_endpoint(
    candidate: &str,
    page_protocol: &str,
) -> Option<ForgeEndpointStorageIntent> {
    if !is_endpoint_bearing_page(page_protocol) {
        return None;
    }

    let endpoint = decode_loopback_forge_endpoint(candidate)?;
    Some(ForgeEndpointStorageIntent::new(endpoint))
}

/// Completes a valid adoption after the caller observes its storage write.
///
/// A failed write is represented as [`ForgeEndpointStorageWriteOutcome::FailureAbsorbed`]
/// and does not change the adopted endpoint.
#[must_use]
pub fn complete_forge_endpoint_adoption(
    intent: ForgeEndpointStorageIntent,
    write: ForgeEndpointStorageWriteResult,
) -> ForgeEndpointAdoptionResult {
    let storage = match write {
        ForgeEndpointStorageWriteResult::Succeeded => ForgeEndpointStorageWriteOutcome::Persisted,
        ForgeEndpointStorageWriteResult::Failed => {
            ForgeEndpointStorageWriteOutcome::FailureAbsorbed
        }
    };

    ForgeEndpointAdoptionResult {
        endpoint: intent.value,
        storage,
    }
}

/// Resolves one storage read to a validated normalized endpoint.
///
/// Missing values, read failures, and invalid or stale stored strings all
/// resolve to `None`. Storage repair or removal, if desired by a future
/// adapter, is outside this reached TypeScript contract.
#[must_use]
pub fn resolve_forge_endpoint(read: ForgeEndpointStorageReadResult) -> Option<String> {
    match read {
        ForgeEndpointStorageReadResult::Value(value) => decode_loopback_forge_endpoint(&value),
        ForgeEndpointStorageReadResult::Missing | ForgeEndpointStorageReadResult::ReadFailure => {
            None
        }
    }
}

/// Assembles an HTTP URL against an adopted origin.
///
/// Without an origin, `path` is copied exactly. With one, this function does
/// only the source policy's string concatenation; it does not add a slash,
/// resolve dot segments, or otherwise normalize the path.
#[must_use]
pub fn forge_http_url(path: &str, adopted_endpoint: Option<&str>) -> String {
    match adopted_endpoint {
        Some(endpoint) => {
            let mut url = String::with_capacity(endpoint.len() + path.len());
            url.push_str(endpoint);
            url.push_str(path);
            url
        }
        None => path.to_owned(),
    }
}

fn candidate_length_is_valid(candidate: &str) -> bool {
    let code_units = candidate.encode_utf16().count();
    (1..=MAX_FORGE_ENDPOINT_CODE_UNITS).contains(&code_units)
}

fn parse_authority(authority: &str) -> Option<(String, String)> {
    if authority.is_empty() || authority.contains('@') {
        return None;
    }

    if authority.starts_with('[') {
        let closing_bracket = authority.find(']')?;
        let host = authority.get(1..closing_bracket)?;
        let remainder = authority.get(closing_bracket + 1..)?;
        let port_text = remainder.strip_prefix(':')?;
        if remainder.len() == 1 || host.parse::<Ipv6Addr>().ok()? != Ipv6Addr::LOCALHOST {
            return None;
        }

        let port = parse_non_default_port(port_text)?;
        return Some((String::from("[::1]"), port));
    }

    let separator = authority.rfind(':')?;
    let host = authority.get(..separator)?;
    let port_text = authority.get(separator + 1..)?;
    if host.is_empty() || host.contains(':') || host != "127.0.0.1" {
        return None;
    }

    let port = parse_non_default_port(port_text)?;
    Some((String::from("127.0.0.1"), port))
}

fn parse_non_default_port(port_text: &str) -> Option<String> {
    if port_text.is_empty() || !port_text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }

    let mut port = 0_u32;
    for digit in port_text.bytes() {
        port = port.checked_mul(10)?.checked_add(u32::from(digit - b'0'))?;
    }
    if port > u16::MAX.into() || port == HTTP_DEFAULT_PORT {
        return None;
    }

    Some(port.to_string())
}
