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
/// The JavaScript source reads hostname and pathname from a WHATWG URL. The
/// adapter therefore performs the URL work needed by the reached special
/// schemes: backslashes become slashes, host names are canonicalized, path
/// bytes are escaped, and dot segments are removed. Query strings, fragments,
/// credentials, and ports never leak into a displayed label.
fn parse_url(web_url: &str) -> Option<ParsedUrl> {
    let sanitized = remove_url_tabs_and_newlines(web_url);
    let web_url = trim_url_whitespace(&sanitized);
    let scheme_end = web_url.find(':')?;
    let scheme = &web_url[..scheme_end];
    if !valid_scheme(scheme) {
        return None;
    }

    let rest = &web_url[scheme_end + 1..];
    if special_scheme(scheme) {
        // Special schemes treat backslashes as slash delimiters and consume
        // all leading slash delimiters while finding the authority.
        let rest = rest.replace('\\', "/");
        let rest = rest.trim_start_matches('/');
        parse_hierarchical_url(rest)
    } else {
        let rest = rest.strip_prefix("//")?;
        parse_hierarchical_url(rest)
    }
}

fn remove_url_tabs_and_newlines(value: &str) -> String {
    value
        .chars()
        .filter(|&character| !matches!(character, '\t' | '\n' | '\r'))
        .collect()
}

fn trim_url_whitespace(value: &str) -> &str {
    value.trim_matches(|character: char| character <= ' ')
}

fn parse_hierarchical_url(rest: &str) -> Option<ParsedUrl> {
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
        normalize_pathname(pathname)
    } else {
        return None;
    };

    Some(ParsedUrl { hostname, pathname })
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
    if host_port.is_empty() {
        return None;
    }

    if host_port.starts_with('[') {
        let closing_bracket = host_port.find(']')?;
        let inner = &host_port[1..closing_bracket];
        if inner.is_empty() || inner.contains(['[', ']', ' ', '\t', '\n', '\r', '\0']) {
            return None;
        }
        let after_bracket = &host_port[closing_bracket + 1..];
        if !valid_port_suffix(after_bracket) {
            return None;
        }
        return Some(format!("[{}]", inner.to_ascii_lowercase()));
    }

    let (hostname, port) = match host_port.rfind(':') {
        Some(colon) => {
            let hostname = &host_port[..colon];
            let port = &host_port[colon + 1..];
            if hostname.contains(':') {
                return None;
            }
            (hostname, Some(port))
        }
        None => (host_port, None),
    };
    if let Some(port) = port
        && !valid_port(port)
    {
        return None;
    }
    if hostname.is_empty() {
        return None;
    }

    let hostname = decode_host_percent(hostname)?;
    if hostname
        .chars()
        .any(|character| character.is_whitespace() || character.is_control())
    {
        return None;
    }
    if hostname.contains(['/', '?', '#', '@', '[', ']', '\\', ':']) {
        return None;
    }

    hostname_to_ascii(&hostname)
}

fn decode_host_percent(hostname: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(hostname.len());
    let mut characters = hostname.chars();
    while let Some(character) = characters.next() {
        if character == '%' {
            let high = hex_value(characters.next()?)?;
            let low = hex_value(characters.next()?)?;
            bytes.push((high << 4) | low);
        } else {
            let mut buffer = [0; 4];
            bytes.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
        }
    }
    String::from_utf8(bytes).ok()
}

fn hex_value(character: char) -> Option<u8> {
    let value = match character {
        '0'..='9' => u32::from(character) - u32::from('0'),
        'a'..='f' => u32::from(character) - u32::from('a') + 10,
        'A'..='F' => u32::from(character) - u32::from('A') + 10,
        _ => return None,
    };
    u8::try_from(value).ok()
}

fn hostname_to_ascii(hostname: &str) -> Option<String> {
    let normalized_dots: String = hostname
        .chars()
        .map(|character| match character {
            '\u{3002}' | '\u{FF0E}' | '\u{FF61}' => '.',
            _ => character,
        })
        .collect();
    let labels: Vec<&str> = normalized_dots.split('.').collect();
    let mut ascii = String::with_capacity(normalized_dots.len());
    for (index, label) in labels.iter().enumerate() {
        if index > 0 {
            ascii.push('.');
        }
        ascii.push_str(&punycode_label(label)?);
    }
    Some(ascii)
}

fn punycode_label(label: &str) -> Option<String> {
    const BASE: u32 = 36;
    const INITIAL_BIAS: u32 = 72;
    const INITIAL_N: u32 = 128;
    const TMAX: u32 = 26;
    const TMIN: u32 = 1;

    let label = label.to_lowercase();
    let codepoints: Vec<u32> = label.chars().map(u32::from).collect();
    let codepoint_count = u32::try_from(codepoints.len()).ok()?;
    let basic_count = codepoints
        .iter()
        .filter(|&&codepoint| codepoint < 0x80)
        .count();
    let basic_count = u32::try_from(basic_count).ok()?;
    let mut output = String::new();
    for &codepoint in &codepoints {
        if codepoint < 0x80 {
            output.push(char::from_u32(codepoint)?);
        }
    }
    if basic_count == codepoint_count {
        return Some(output);
    }
    output.insert_str(0, "xn--");
    if basic_count > 0 {
        output.push('-');
    }

    let mut handled = basic_count;
    let mut next_codepoint = INITIAL_N;
    let mut delta = 0_u64;
    let mut bias = INITIAL_BIAS;

    while handled < codepoint_count {
        let next = codepoints
            .iter()
            .copied()
            .filter(|&codepoint| codepoint >= next_codepoint)
            .min()?;
        let increment =
            u64::from(next - next_codepoint).checked_mul(u64::from(handled).checked_add(1)?)?;
        delta = delta.checked_add(increment)?;
        next_codepoint = next;

        for &codepoint in &codepoints {
            if codepoint < next_codepoint {
                delta = delta.checked_add(1)?;
            }
            if codepoint == next_codepoint {
                let mut quotient = delta;
                let mut divisor = BASE;
                loop {
                    let threshold = if divisor <= bias {
                        TMIN
                    } else if divisor >= bias + TMAX {
                        TMAX
                    } else {
                        divisor - bias
                    };
                    if quotient < u64::from(threshold) {
                        break;
                    }
                    let base_minus_threshold = BASE - threshold;
                    let remainder =
                        (quotient - u64::from(threshold)) % u64::from(base_minus_threshold);
                    let digit = threshold.checked_add(u32::try_from(remainder).ok()?)?;
                    output.push(encode_punycode_digit(digit)?);
                    quotient = (quotient - u64::from(threshold)) / u64::from(base_minus_threshold);
                    divisor = divisor.checked_add(BASE)?;
                }
                output.push(encode_punycode_digit(u32::try_from(quotient).ok()?)?);
                bias = adapt_punycode_bias(delta, handled + 1, handled == basic_count);
                delta = 0;
                handled = handled.checked_add(1)?;
            }
        }
        delta = delta.checked_add(1)?;
        next_codepoint = next_codepoint.checked_add(1)?;
    }

    Some(output)
}

fn encode_punycode_digit(digit: u32) -> Option<char> {
    match digit {
        0..=25 => char::from_u32(u32::from(b'a') + digit),
        26..=35 => char::from_u32(u32::from(b'0') + digit - 26),
        _ => None,
    }
}

fn adapt_punycode_bias(delta: u64, number_handled: u32, first: bool) -> u32 {
    const DAMP: u64 = 700;
    const SKEW: u32 = 38;

    let mut delta = if first { delta / DAMP } else { delta / 2 };
    delta += delta / u64::from(number_handled);
    let mut bias = 0_u32;
    while delta > 455 {
        delta /= 35;
        bias += 36;
    }
    let scaled = u32::try_from((delta * 36) / (delta + u64::from(SKEW))).unwrap_or(u32::MAX);
    bias + scaled
}

fn normalize_pathname(pathname: &str) -> String {
    let encoded = percent_encode_path(pathname);
    let segments: Vec<&str> = encoded.split('/').collect();
    let mut normalized = Vec::with_capacity(segments.len());
    for (index, segment) in segments.iter().enumerate() {
        let is_last = index + 1 == segments.len();
        if is_single_dot_segment(segment) {
            if is_last {
                normalized.push("");
            }
        } else if is_double_dot_segment(segment) {
            if normalized.len() > 1 {
                normalized.pop();
            }
            if is_last {
                normalized.push("");
            }
        } else {
            normalized.push(segment);
        }
    }
    normalized.join("/")
}

fn is_single_dot_segment(segment: &str) -> bool {
    segment == "." || segment.eq_ignore_ascii_case("%2e")
}

fn is_double_dot_segment(segment: &str) -> bool {
    segment == ".."
        || segment.eq_ignore_ascii_case("%2e.")
        || segment.eq_ignore_ascii_case(".%2e")
        || segment.eq_ignore_ascii_case("%2e%2e")
}

fn percent_encode_path(pathname: &str) -> String {
    let mut encoded = String::with_capacity(pathname.len());
    for character in pathname.chars() {
        if matches!(character, '\t' | '\n' | '\r') {
            continue;
        }
        if character.is_ascii() {
            let byte = u8::try_from(u32::from(character)).unwrap_or_default();
            if path_byte_requires_encoding(byte) {
                push_percent_encoded_byte(&mut encoded, byte);
            } else {
                encoded.push(character);
            }
        } else {
            let mut buffer = [0; 4];
            for &byte in character.encode_utf8(&mut buffer).as_bytes() {
                push_percent_encoded_byte(&mut encoded, byte);
            }
        }
    }
    encoded
}

fn path_byte_requires_encoding(byte: u8) -> bool {
    byte <= 0x20
        || byte == 0x7f
        || matches!(byte, b'"' | b'<' | b'>' | b'^' | b'\x60' | b'{' | b'}')
}

fn push_percent_encoded_byte(output: &mut String, byte: u8) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    output.push('%');
    output.push(char::from(HEX[usize::from(byte >> 4)]));
    output.push(char::from(HEX[usize::from(byte & 0x0f)]));
}

fn valid_port_suffix(suffix: &str) -> bool {
    suffix.is_empty() || (suffix.starts_with(':') && valid_port(&suffix[1..]))
}

fn valid_port(port: &str) -> bool {
    port.is_empty()
        || (port.bytes().all(|byte| byte.is_ascii_digit())
            && port.parse::<u32>().is_ok_and(|value| value <= 65_535))
}
