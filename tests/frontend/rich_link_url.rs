//! Focused coverage for the rich-link URL selection and canonicalization policy.

use artisan_frontend::rich_link_url::rich_link_metadata_url;

#[test]
fn missing_relative_protocol_relative_and_malformed_values_are_rejected() {
    let rejected: &[Option<&str>] = &[
        None,
        Some(""),
        Some(" "),
        Some("example.com/path"),
        Some("/relative/path"),
        Some("//example.com/path"),
        Some("http://"),
        Some("https://[2001:db8::1"),
    ];

    for href in rejected {
        assert_eq!(
            rich_link_metadata_url(*href),
            None,
            "rich-link policy must reject {href:?}"
        );
    }
}

#[test]
fn only_exact_http_and_https_schemes_are_accepted() {
    let rejected = [
        "ftp://example.com/file",
        "mailto:user@example.com",
        "javascript:alert(1)",
        "data:text/html,not-a-link",
        "httpx://example.com",
        "httpsx://example.com",
        "javascript://https://example.com",
    ];

    for href in rejected {
        assert_eq!(
            rich_link_metadata_url(Some(href)),
            None,
            "deceptive or unsupported scheme must be rejected: {href}"
        );
    }
}

#[test]
fn supported_urls_use_browser_compatible_canonical_serialization() {
    let cases = [
        ("HTTP://Example.COM", "http://example.com/"),
        ("hTtPs://Example.COM:443/path", "https://example.com/path"),
        ("HTTP://Example.COM:80/path", "http://example.com/path"),
        (
            "https://User:Pass@Example.COM:8443/a/b?x=1&y=two#section",
            "https://User:Pass@example.com:8443/a/b?x=1&y=two#section",
        ),
        (
            "https://example.com/a/../b?query=value#fragment",
            "https://example.com/b?query=value#fragment",
        ),
    ];

    for (href, expected) in cases {
        assert_eq!(
            rich_link_metadata_url(Some(href)).as_deref(),
            Some(expected),
            "canonical URL for {href}"
        );
    }
}
