//! Pure machine-switch validation and navigation policy.
//!
//! This is the native counterpart of
//! `modules/frontend/src/lib/identity/machine-switch.ts`. Browser
//! `sessionStorage`, JSON decoding, Effect execution, and the eventual
//! navigation side effect remain outside this module. The optional string
//! inputs to [`validate_home_host_memory`] represent fields that may be
//! absent or rejected by the dynamic browser-storage decoder.

/// The remembered identity of the Forge that a document switched away from.
///
/// A valid memory always has a non-empty `label`. An empty or absent `detail`
/// is normalized to `None`, matching the TypeScript recall policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HomeHostMemory {
    /// Optional host name or other secondary display detail.
    pub detail: Option<String>,
    /// Required display label.
    pub label: String,
}

impl HomeHostMemory {
    /// Validates the typed fields of one remembered home host.
    ///
    /// `None` for `label` represents a missing or non-string value at the
    /// dynamic storage boundary and is rejected. The TypeScript policy checks
    /// length rather than trimming, so whitespace-only strings remain valid.
    /// `detail` is retained only when it is present and non-empty.
    #[must_use]
    pub fn validate(label: Option<&str>, detail: Option<&str>) -> Option<Self> {
        let label = label?;
        if label.is_empty() {
            return None;
        }

        Some(Self {
            detail: detail.filter(|value| !value.is_empty()).map(str::to_owned),
            label: label.to_owned(),
        })
    }
}

/// Validates and normalizes a remembered home host.
///
/// This free-function form keeps the policy convenient for a decoder that has
/// already extracted the two fields from a dynamic value.
#[must_use]
pub fn validate_home_host_memory(
    label: Option<&str>,
    detail: Option<&str>,
) -> Option<HomeHostMemory> {
    HomeHostMemory::validate(label, detail)
}

const URI_COMPONENT_HEX: &[u8; 16] = b"0123456789ABCDEF";

const fn is_uri_component_unescaped(byte: u8) -> bool {
    matches!(
        byte,
        b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')'
    )
}

/// Encodes a valid UTF-8 string with JavaScript `encodeURIComponent` rules.
///
/// The punctuation characters that `encodeURIComponent` leaves alone in
/// addition to alphanumeric characters are `- _ . ! ~ * ' ( )`. Every other
/// UTF-8 byte is emitted as an uppercase hexadecimal `%XX` escape. This is
/// intentionally dependency-free and does not turn spaces into `+`.
#[must_use]
pub fn encode_uri_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if is_uri_component_unescaped(byte) {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(URI_COMPONENT_HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(URI_COMPONENT_HEX[usize::from(byte & 0x0f)]));
        }
    }
    encoded
}

/// Builds the pure URL used to switch the current document to another Forge.
///
/// HTTP and HTTPS pages navigate to the endpoint's origin and carry only the
/// encoded pair code in the fragment. Every other page scheme uses the
/// current page origin, injects the caller-supplied handoff query parameter,
/// and appends the pair/Forge fragment in that exact order. Only one trailing
/// slash is removed from the endpoint in the HTTP(S) branch; no other URL
/// normalization is performed. The desktop/non-HTTP nonce is interpolated
/// byte-for-byte, while the pair code and endpoint are URI-component encoded.
///
/// Rust's protocol module does not currently expose the shared
/// `desktop_handoff_navigation_parameter` constant. Until it does, callers
/// inject that value here (the current TypeScript protocol value is
/// `"artisan-handoff"`).
#[must_use]
pub fn build_machine_switch_url(
    page_protocol: &str,
    page_origin: &str,
    endpoint: &str,
    pair_code: &str,
    nonce: &str,
    handoff_navigation_parameter: &str,
) -> String {
    if matches!(page_protocol, "http:" | "https:") {
        let origin = endpoint.strip_suffix('/').unwrap_or(endpoint);
        return format!("{origin}/#pair={}", encode_uri_component(pair_code));
    }

    format!(
        "{page_origin}/?{handoff_navigation_parameter}={}#pair={}&forge={}",
        nonce,
        encode_uri_component(pair_code),
        encode_uri_component(endpoint),
    )
}
