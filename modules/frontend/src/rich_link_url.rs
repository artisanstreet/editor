//! Pure URL policy for rich-link metadata.
//!
//! The browser implementation in
//! `modules/frontend/src/lib/components/markdown/link-url.ts` parses the
//! supplied value without a base URL, permits only `http:` and `https:`, and
//! returns the browser URL's canonical `href`. This module keeps that policy
//! free of fetching, metadata, and rendering concerns.

#![allow(clippy::module_name_repetitions)]

use url::Url;

/// Parses and canonicalizes an absolute HTTP(S) URL for rich-link metadata.
///
/// Missing, relative, protocol-relative, malformed, and non-HTTP(S) values
/// return `None`. The URL parser supplies the scheme/host normalization and
/// serialization, including default-port removal, path escaping, and the
/// preservation of credentials, queries, and fragments.
#[must_use]
pub fn rich_link_metadata_url(href: Option<&str>) -> Option<String> {
    let href = href?;
    let url = Url::parse(href).ok()?;

    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return None;
    }

    Some(url.as_str().to_owned())
}
