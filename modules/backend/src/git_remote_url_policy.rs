//! Dependency-free parsing and projection policy for Git remote URLs.
//!
//! This is the native counterpart of `git/remote-url.ts`. Git reports
//! remotes in several syntaxes, so the policy first reduces each input to a
//! hostname, a normalized path, and whether it names a network remote. Host
//! classification and browser projection then operate on those same parts.
//! URL parsing is intentionally local to this module: the surrounding Git
//! service, protocol, and process boundaries do not belong here.

#![allow(clippy::module_name_repetitions)]
#![forbid(unsafe_code)]

use std::fmt;

/// The service or custody class identified from a Git remote hostname.
///
/// `Other` means that the remote has a usable network hostname but does not
/// match one of the known service families. `Unknown` means that parsing did
/// not produce a browsable network host, including filesystem remotes and
/// malformed or empty input.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RepositoryHost {
    /// Azure DevOps (`*.dev.azure.com` and `*.visualstudio.com`).
    Azure,
    /// Bitbucket (`*.bitbucket.org`).
    Bitbucket,
    /// Codeberg (`*.codeberg.org`).
    Codeberg,
    /// Gitea, including self-hosted names containing a `gitea.` label.
    Gitea,
    /// GitHub (`*.github.com`).
    Github,
    /// GitLab, including self-hosted names containing a `gitlab.` label.
    Gitlab,
    /// A network host with no recognized service family.
    Other,
    /// `SourceHut` (`*.sr.ht`).
    Sourcehut,
    /// No usable network hostname was present.
    Unknown,
}

impl RepositoryHost {
    /// The protocol spelling used by the adjacent TypeScript boundary.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Azure => "azure",
            Self::Bitbucket => "bitbucket",
            Self::Codeberg => "codeberg",
            Self::Gitea => "gitea",
            Self::Github => "github",
            Self::Gitlab => "gitlab",
            Self::Other => "other",
            Self::Sourcehut => "sourcehut",
            Self::Unknown => "unknown",
        }
    }

    /// Mixed-case alias for the GitHub product name.
    #[allow(non_upper_case_globals)]
    pub const GitHub: Self = Self::Github;

    /// Mixed-case alias for the GitLab product name.
    #[allow(non_upper_case_globals)]
    pub const GitLab: Self = Self::Gitlab;

    /// Mixed-case alias for the `SourceHut` product name.
    #[allow(non_upper_case_globals)]
    pub const SourceHut: Self = Self::Sourcehut;
}

impl fmt::Display for RepositoryHost {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Eq, PartialEq)]
struct RemoteParts {
    hostname: String,
    path: String,
    networked: bool,
}

#[derive(Debug, Eq, PartialEq)]
struct UrlParts {
    protocol: String,
    hostname: String,
    pathname: String,
}

/// Trims the ECMAScript whitespace set used by `String.prototype.trim`.
fn javascript_trim(value: &str) -> &str {
    value.trim_matches(|character: char| {
        matches!(
            character,
            '\u{0009}'..='\u{000d}'
                | '\u{0020}'
                | '\u{00a0}'
                | '\u{1680}'
                | '\u{2000}'..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
        )
    })
}

/// Removes leading and trailing slashes, then one lower-case `.git` suffix.
fn normalize_path(path: &str) -> String {
    let without_slashes = path.trim_start_matches('/').trim_end_matches('/');
    without_slashes
        .strip_suffix(".git")
        .unwrap_or(without_slashes)
        .to_owned()
}

/// Returns whether a path satisfies the JavaScript `scp`-like expression's
/// `[^\\].*` tail. The first character may be a line terminator, but the
/// expression's dot cannot consume one afterward.
fn scp_path_matches(path: &str) -> bool {
    let mut characters = path.chars();
    let Some(first) = characters.next() else {
        return false;
    };

    first != '\\' && characters.all(|character| !is_javascript_line_terminator(character))
}

fn is_javascript_line_terminator(character: char) -> bool {
    matches!(character, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// Matches the legacy `scp`-like remote shape without a regular-expression
/// dependency.
fn parse_scp_like(value: &str) -> Option<(&str, &str)> {
    if value.contains("://") {
        return None;
    }

    let valid_hostname = |hostname: &str| {
        !hostname.is_empty()
            && !hostname.contains(':')
            && !hostname.contains('/')
            && !hostname.contains('\\')
    };

    // The optional user group is attempted first, just as in the source
    // regular expression. If that attempt cannot complete a full match, the
    // expression can still backtrack and match the no-user form.
    if let Some(at) = value.find('@') {
        let user = &value[..at];
        if !user.is_empty() && !user.contains('/') && !user.contains('\\') {
            let after_at = &value[at + 1..];
            if let Some(delimiter_after_at) = after_at.find(':') {
                let delimiter = at + 1 + delimiter_after_at;
                let hostname = &value[at + 1..delimiter];
                let path = &value[delimiter + 1..];
                if valid_hostname(hostname) && scp_path_matches(path) {
                    return Some((hostname, path));
                }
            }
        }
    }

    let delimiter = value.find(':')?;
    let hostname = &value[..delimiter];
    let path = &value[delimiter + 1..];

    (valid_hostname(hostname) && scp_path_matches(path)).then_some((hostname, path))
}

fn is_windows_path(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\')
}

fn is_ascii_scheme(value: &str) -> bool {
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };

    first.is_ascii_alphabetic()
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
}

fn is_special_url_scheme(protocol: &str) -> bool {
    matches!(protocol, "ftp" | "file" | "http" | "https" | "ws" | "wss")
}

fn first_url_delimiter(value: &str) -> usize {
    value.find(['/', '?', '#']).unwrap_or(value.len())
}

fn path_before_query_or_fragment(value: &str) -> &str {
    let end = value.find(['?', '#']).unwrap_or(value.len());
    &value[..end]
}

fn validate_hostname(hostname: &str, special: bool) -> Option<String> {
    let bracketed_ipv6 = hostname.starts_with('[') && hostname.ends_with(']');

    if hostname.is_empty()
        || hostname
            .chars()
            .any(|character| character.is_ascii_control() || character.is_whitespace())
        || hostname.contains(['/', '\\'])
        || (!bracketed_ipv6 && hostname.contains(['[', ']']))
        || (bracketed_ipv6 && hostname[1..hostname.len() - 1].is_empty())
        || (special && hostname.contains('%'))
    {
        return None;
    }

    Some(hostname.to_lowercase())
}

fn parse_port(port: &str) -> Option<()> {
    if port.is_empty() {
        return Some(());
    }

    if !port.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }

    port.parse::<u32>()
        .is_ok_and(|port| u16::try_from(port).is_ok())
        .then_some(())
}

fn parse_authority_host(authority: &str, special: bool) -> Option<String> {
    let host_and_port = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host_and_port)| host_and_port);

    if host_and_port.starts_with('[') {
        let closing = host_and_port.find(']')?;
        let hostname = &host_and_port[..=closing];
        let remainder = &host_and_port[closing + 1..];

        if !remainder.is_empty() && !remainder.starts_with(':') {
            return None;
        }
        parse_port(remainder.strip_prefix(':').unwrap_or_default())?;
        return validate_hostname(hostname, special);
    }

    let Some((hostname, port)) = host_and_port.rsplit_once(':') else {
        return validate_hostname(host_and_port, special);
    };

    if hostname.contains(':') {
        return None;
    }
    parse_port(port)?;
    validate_hostname(hostname, special)
}

/// Applies URL path dot-segment reduction while retaining repeated slashes.
fn remove_url_dot_segments(path: &str) -> String {
    let ends_in_dot_segment = path.ends_with("/.") || path.ends_with("/..");
    let mut segments: Vec<&str> = Vec::new();

    for segment in path.split('/') {
        match segment {
            "." => {}
            ".." => {
                if segments.last().is_some_and(|last| !last.is_empty()) {
                    segments.pop();
                }
            }
            _ => segments.push(segment),
        }
    }

    let mut reduced = segments.join("/");
    if ends_in_dot_segment && !reduced.ends_with('/') {
        reduced.push('/');
    }
    reduced
}

fn parse_hierarchical_url(protocol: String, rest: &str) -> Option<UrlParts> {
    let after_authority_marker = &rest[2..];
    let special = is_special_url_scheme(&protocol);

    // WHATWG special URLs treat one or more extra slashes before a host as
    // authority separators (`https:///owner/repo` has host `owner`). `file:`
    // is different: its empty authority denotes a local filesystem path.
    let authority_source = if special && protocol != "file" {
        after_authority_marker.trim_start_matches('/')
    } else {
        after_authority_marker
    };
    let authority_end = first_url_delimiter(authority_source);
    let authority = &authority_source[..authority_end];
    let tail = &authority_source[authority_end..];

    if special && protocol != "file" && authority.is_empty() {
        return None;
    }

    let hostname = if authority.is_empty() {
        String::new()
    } else {
        parse_authority_host(authority, special)?
    };
    let raw_path = path_before_query_or_fragment(tail);
    let pathname = if raw_path.is_empty() {
        "/".to_owned()
    } else {
        remove_url_dot_segments(raw_path)
    };

    Some(UrlParts {
        protocol,
        hostname,
        pathname,
    })
}

fn parse_url(value: &str) -> Option<UrlParts> {
    let scheme_end = value.find(':')?;
    let scheme = &value[..scheme_end];
    if !is_ascii_scheme(scheme) {
        return None;
    }

    let protocol = scheme.to_ascii_lowercase();
    let special = is_special_url_scheme(&protocol);
    let transformed = if special && value.contains('\\') {
        Some(value.replace('\\', "/"))
    } else {
        None
    };
    let input = transformed.as_deref().unwrap_or(value);
    let rest = &input[scheme_end + 1..];

    if rest.starts_with("//") {
        return parse_hierarchical_url(protocol, rest);
    }

    if special {
        if protocol == "file" {
            let pathname = if rest.is_empty() {
                "/".to_owned()
            } else if rest.starts_with('/') {
                path_before_query_or_fragment(rest).to_owned()
            } else {
                format!("/{rest}")
            };
            return Some(UrlParts {
                protocol,
                hostname: String::new(),
                pathname,
            });
        }

        if rest.starts_with(['?', '#']) {
            return None;
        }

        let authority = rest.trim_start_matches('/');
        return parse_hierarchical_url(protocol, &format!("//{authority}"));
    }

    Some(UrlParts {
        protocol,
        hostname: String::new(),
        pathname: path_before_query_or_fragment(rest).to_owned(),
    })
}

fn parse_remote(value: &str) -> Option<RemoteParts> {
    let trimmed = javascript_trim(value);
    if trimmed.is_empty() {
        return None;
    }

    if is_windows_path(trimmed) || trimmed.starts_with('/') || trimmed.starts_with('.') {
        return Some(RemoteParts {
            hostname: String::new(),
            networked: false,
            path: normalize_path(trimmed),
        });
    }

    if let Some(local_path) = trimmed.strip_prefix("file://") {
        return Some(RemoteParts {
            hostname: String::new(),
            networked: false,
            path: normalize_path(local_path),
        });
    }

    if let Some((hostname, path)) = parse_scp_like(trimmed) {
        return Some(RemoteParts {
            hostname: hostname.to_lowercase(),
            networked: true,
            path: normalize_path(path),
        });
    }

    let parsed = parse_url(trimmed)?;
    Some(RemoteParts {
        networked: parsed.protocol != "file" && !parsed.hostname.is_empty(),
        hostname: parsed.hostname,
        path: normalize_path(&parsed.pathname),
    })
}

fn exact_or_subdomain(hostname: &str, suffix: &str) -> bool {
    hostname == suffix
        || hostname
            .strip_suffix(suffix)
            .is_some_and(|prefix| prefix.ends_with('.'))
}

fn label_before_dot(hostname: &str, label: &str) -> bool {
    hostname.starts_with(&format!("{label}.")) || hostname.contains(&format!(".{label}."))
}

fn classify_hostname(hostname: &str) -> RepositoryHost {
    if exact_or_subdomain(hostname, "github.com") {
        RepositoryHost::Github
    } else if label_before_dot(hostname, "gitlab") {
        RepositoryHost::Gitlab
    } else if exact_or_subdomain(hostname, "bitbucket.org") {
        RepositoryHost::Bitbucket
    } else if exact_or_subdomain(hostname, "dev.azure.com")
        || exact_or_subdomain(hostname, "visualstudio.com")
    {
        RepositoryHost::Azure
    } else if exact_or_subdomain(hostname, "codeberg.org") {
        RepositoryHost::Codeberg
    } else if exact_or_subdomain(hostname, "sr.ht") {
        RepositoryHost::Sourcehut
    } else if label_before_dot(hostname, "gitea") {
        RepositoryHost::Gitea
    } else {
        RepositoryHost::Other
    }
}

/// Identifies the service hosting a remote.
///
/// Empty, malformed, and local/file remotes return [`RepositoryHost::Unknown`].
/// A valid network hostname that matches no known service returns
/// [`RepositoryHost::Other`].
#[must_use]
pub fn repository_host_for(url: &str) -> RepositoryHost {
    let Some(parts) = parse_remote(url) else {
        return RepositoryHost::Unknown;
    };

    if parts.networked {
        classify_hostname(&parts.hostname)
    } else {
        RepositoryHost::Unknown
    }
}

/// Derives the HTTPS page a browser should open for a Git remote.
///
/// Network remotes with a non-empty normalized path use the hostname and path
/// directly. Azure DevOps SSH paths of the exact form
/// `v3/organization/project/repository` are rewritten to the corresponding
/// `organization/project/_git/repository` web path. All other inputs return
/// `None` because they do not identify a browsable repository page.
#[must_use]
pub fn repository_web_url_for(url: &str) -> Option<String> {
    let parts = parse_remote(url)?;
    if !parts.networked || parts.path.is_empty() {
        return None;
    }

    if classify_hostname(&parts.hostname) == RepositoryHost::Azure {
        let segments: Vec<&str> = parts
            .path
            .split('/')
            .filter(|segment| !segment.is_empty())
            .collect();

        if segments.first() == Some(&"v3") && segments.len() == 4 {
            return Some(format!(
                "https://{}/{}/{}/_git/{}",
                parts.hostname, segments[1], segments[2], segments[3]
            ));
        }
    }

    Some(format!("https://{}/{}", parts.hostname, parts.path))
}
