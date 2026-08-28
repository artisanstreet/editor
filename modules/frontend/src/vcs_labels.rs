//! Pure repository URL labels for frontend VCS surfaces.
//!
//! This is the native counterpart of `modules/frontend/src/lib/vcs/labels.ts`.
//! The source module only presents a repository's already-derived web URL; it
//! does not inspect Git state or perform any I/O. The three helpers below keep
//! that same narrow presentation boundary and preserve the source's fallback
//! and output ordering rules.

/// Reduces a repository web URL to the repository's own name.
///
/// The owner is dropped because a repository row is already scoped to one
/// project. A URL with no path falls back to its host, while an input that the
/// JavaScript `URL` constructor would reject is returned unchanged.
///
/// This mirrors `RepositoryLinkLabel` in `labels.ts`.
#[must_use]
pub fn repository_link_label(web_url: &str) -> String {
    let Some(parsed) = parse_url(web_url) else {
        return web_url.to_owned();
    };

    let segments = pathname_segments(&parsed.pathname);
    match segments.last() {
        Some(segment) => (*segment).to_owned(),
        None => parsed.hostname,
    }
}

/// Reduces a repository web URL to its innermost `owner/repository` pair.
///
/// Nested groups retain only the final two non-empty path segments, and a URL
/// with one path segment retains that segment alone. A URL with no path falls
/// back to its host. Rejected URL input is returned unchanged.
///
/// This mirrors `RepositoryQualifiedLabel` in `labels.ts`.
#[must_use]
pub fn repository_qualified_label(web_url: &str) -> String {
    let Some(parsed) = parse_url(web_url) else {
        return web_url.to_owned();
    };

    let segments = pathname_segments(&parsed.pathname);
    if segments.is_empty() {
        return parsed.hostname;
    }

    let name_index = segments.len() - 1;
    let name = segments[name_index];
    if name_index == 0 {
        return name.to_owned();
    }

    let owner = segments[name_index - 1];
    format!("{owner}/{name}")
}

/// Names a repository destination without its transport scheme.
///
/// The host remains first, followed by the URL pathname. Exactly one trailing
/// slash is removed from that pathname, matching the source's regular
/// expression. Rejected URL input is returned unchanged.
///
/// This mirrors `RepositoryDestinationLabel` in `labels.ts`.
#[must_use]
pub fn repository_destination_label(web_url: &str) -> String {
    let Some(parsed) = parse_url(web_url) else {
        return web_url.to_owned();
    };

    let pathname = parsed
        .pathname
        .strip_suffix('/')
        .unwrap_or(&parsed.pathname);
    format!("{}{pathname}", parsed.hostname)
}

struct ParsedUrl {
    hostname: String,
    pathname: String,
}

/// Splits a URL pathname exactly as the source does: slash-separated empty
/// segments are discarded before a link or qualified label is selected.
fn pathname_segments(pathname: &str) -> Vec<&str> {
    pathname
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect()
}

/// Parses the absolute hierarchical URLs supplied by the repository protocol.
///
/// The backend supplies HTTPS web URLs, but the JavaScript source does not
/// branch on the scheme, so every valid scheme-shaped hierarchical URL is
/// accepted here. Only the URL fields read by `labels.ts` are retained:
/// hostname and pathname. Query strings, fragments, credentials, and ports
/// therefore never leak into a displayed label.
fn parse_url(web_url: &str) -> Option<ParsedUrl> {
    let web_url = trim_url_whitespace(web_url);
    let scheme_end = web_url.find("://")?;
    let scheme = &web_url[..scheme_end];
    if !valid_scheme(scheme) {
        return None;
    }

    let rest = &web_url[scheme_end + 3..];
    if rest.is_empty() {
        return None;
    }

    // Special URL schemes treat additional leading slashes as part of the
    // authority delimiter. This preserves the browser URL behavior for an
    // input such as `https:///owner/repository`, which resolves to host
    // `owner` and pathname `/repository` rather than throwing.
    let rest = if special_scheme(scheme) {
        rest.trim_start_matches('/')
    } else {
        rest
    };
    if rest.is_empty() {
        return None;
    }

    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if authority.is_empty() {
        return None;
    }

    let host_port = authority.rsplit('@').next()?;
    let hostname = parse_hostname(host_port)?;
    let after_authority = &rest[authority_end..];
    let pathname_end = after_authority
        .find(['?', '#'])
        .unwrap_or(after_authority.len());
    let pathname = &after_authority[..pathname_end];
    let pathname = if pathname.is_empty() {
        "/".to_owned()
    } else if pathname.starts_with('/') {
        pathname.to_owned()
    } else {
        return None;
    };

    Some(ParsedUrl { hostname, pathname })
}

fn trim_url_whitespace(value: &str) -> &str {
    value.trim_matches([' ', '\t', '\n', '\r'])
}

fn valid_scheme(scheme: &str) -> bool {
    let mut characters = scheme.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    first.is_ascii_alphabetic()
        && characters.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.')
        })
}

fn special_scheme(scheme: &str) -> bool {
    ["http", "https", "ftp", "ws", "wss"]
        .iter()
        .any(|candidate| scheme.eq_ignore_ascii_case(candidate))
}

fn parse_hostname(host_port: &str) -> Option<String> {
    if host_port.is_empty() || host_port.contains([' ', '\t', '\n', '\r', '\0']) {
        return None;
    }

    let hostname = if host_port.starts_with('[') {
        let closing_bracket = host_port.find(']')?;
        let after_bracket = &host_port[closing_bracket + 1..];
        if !valid_port_suffix(after_bracket) {
            return None;
        }
        &host_port[..=closing_bracket]
    } else {
        let (hostname, port) = match host_port.split_once(':') {
            Some((hostname, port)) => (hostname, Some(port)),
            None => (host_port, None),
        };
        if host_port[hostname.len() + usize::from(port.is_some())..].contains(':') {
            return None;
        }
        if let Some(port) = port
            && !valid_port(port)
        {
            return None;
        }
        hostname
    };

    if hostname.is_empty() || hostname.contains(['/', '?', '#', '@', '\0']) {
        return None;
    }

    Some(hostname.to_ascii_lowercase())
}

fn valid_port_suffix(suffix: &str) -> bool {
    suffix.is_empty() || (suffix.starts_with(':') && valid_port(&suffix[1..]))
}

fn valid_port(port: &str) -> bool {
    port.is_empty()
        || (port.bytes().all(|byte| byte.is_ascii_digit())
            && port.parse::<u32>().is_ok_and(|value| value <= 65_535))
}
