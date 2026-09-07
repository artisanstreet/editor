//! Bounded child readiness parsing.
//!
//! The child writes exactly one newline-terminated JSON line of the form
//! `{"url": "<nonempty http loopback url>"}`. This module reads that line
//! through the owned async pipe without unbounded allocation, validates byte
//! bounds, trailing data, UTF-8, JSON shape, and URL loopback policy, and
//! exposes the validated loopback endpoint. No payload is ever logged or
//! included in error `Display`.
//!
//! Bounds are caller-supplied; the only allocation limit is the configured
//! `max_readiness_line` checked as `cap + 1` before any buffering.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use thiserror::Error;
use tokio::io::AsyncReadExt;
use tokio::process::ChildStdout;
use tokio::time::Instant;

use artisan_transport::CancelHandle;

/// Typed, payload-free failure while reading or validating readiness.
///
/// Every `Display` is a constant string; no input bytes, URLs, or
/// operating-system messages are surfaced.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
pub enum ReadinessError {
    /// Readiness line was empty after trimming the optional carriage return.
    #[error("readiness line was empty")]
    EmptyLine,

    /// Readiness bytes exceeded the configured limit before a newline.
    #[error("readiness line exceeded the configured limit")]
    Oversized,

    /// The pipe reached EOF before a terminating newline.
    #[error("readiness pipe ended before newline")]
    EofBeforeNewline,

    /// The payload after the optional carriage return contained trailing bytes
    /// beyond the first newline.
    #[error("readiness line had trailing bytes")]
    TrailingBytes,

    /// Readiness bytes were not valid UTF-8.
    #[error("readiness bytes were not valid utf-8")]
    InvalidUtf8,

    /// Readiness bytes were not valid JSON.
    #[error("readiness bytes were not valid json")]
    InvalidJson,

    /// JSON was not the strict `{"url": <string>}` shape.
    #[error("readiness json had unexpected shape")]
    UnexpectedShape,

    /// The `url` string was empty.
    #[error("readiness url was empty")]
    EmptyUrl,

    /// URL scheme was not exactly `http`.
    #[error("readiness url scheme was not http")]
    InvalidScheme,

    /// URL host was not loopback `127.0.0.1` or `::1`.
    #[error("readiness url host was not loopback")]
    InvalidHost,

    /// URL port was missing or not in `1..=65535`.
    #[error("readiness url port was invalid")]
    InvalidPort,

    /// URL contained userinfo or credentials.
    #[error("readiness url contained credentials")]
    CredentialsPresent,

    /// The configured readiness limit overflowed `cap + 1`.
    #[error("readiness cap overflowed")]
    UnrepresentableCap,

    /// The readiness deadline elapsed.
    #[error("readiness deadline elapsed")]
    Deadline,

    /// The operation was cancelled.
    #[error("readiness was cancelled")]
    Cancelled,

    /// The owner is shutting down.
    #[error("owner is shutting down")]
    Shutdown,

    /// The pipe read failed.
    #[error("readiness pipe read failed")]
    Io,
}

/// Validated readiness endpoint.
///
/// The original URL string is preserved verbatim; host and port are
/// separately validated loopback values.
pub struct ValidatedEndpoint {
    raw_url: String,
    host: IpAddr,
    port: u16,
}

impl ValidatedEndpoint {
    /// Returns the original URL string as supplied by the child.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.raw_url
    }

    /// Returns the validated loopback host.
    #[must_use]
    pub fn host(&self) -> IpAddr {
        self.host
    }

    /// Returns the validated port.
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Returns the socket address for `TcpStream::connect`.
    #[must_use]
    pub fn socket_addr(&self) -> SocketAddr {
        SocketAddr::new(self.host, self.port)
    }
}

impl fmt::Debug for ValidatedEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ValidatedEndpoint")
            .field("raw_url", &"<redacted>")
            .field("host", &self.host)
            .field("port", &self.port)
            .finish()
    }
}

/// Validates a URL string as loopback `http` with no credentials.
///
/// No allocation beyond the returned endpoint; no network probe is performed.
fn validate_url(url: &str) -> Result<ValidatedEndpoint, ReadinessError> {
    if url.is_empty() {
        return Err(ReadinessError::EmptyUrl);
    }
    if url.contains('@') {
        return Err(ReadinessError::CredentialsPresent);
    }
    let Some(remainder) = url.strip_prefix("http://") else {
        return Err(ReadinessError::InvalidScheme);
    };
    if remainder.is_empty() {
        return Err(ReadinessError::InvalidHost);
    }

    let (host, port_str, _suffix) = if remainder.starts_with('[') {
        // IPv6 bracketed form: [::1]:port[/...]
        let close = remainder.find(']').ok_or(ReadinessError::InvalidHost)?;
        let host_text = &remainder[1..close];
        if host_text != "::1" {
            return Err(ReadinessError::InvalidHost);
        }
        let after = &remainder[close + 1..];
        let Some(port_part) = after.strip_prefix(':') else {
            return Err(ReadinessError::InvalidPort);
        };
        let end = port_part
            .find('/')
            .or_else(|| port_part.find('?'))
            .or_else(|| port_part.find('#'))
            .unwrap_or(port_part.len());
        let port_str = &port_part[..end];
        let suffix = &port_part[end..];
        if port_str.is_empty() {
            return Err(ReadinessError::InvalidPort);
        }
        let host = IpAddr::V6(Ipv6Addr::LOCALHOST);
        (host, port_str, suffix)
    } else {
        // IPv4 form: 127.0.0.1:port[/...]
        let prefix = "127.0.0.1";
        if !remainder.starts_with(prefix) {
            return Err(ReadinessError::InvalidHost);
        }
        let after = &remainder[prefix.len()..];
        let Some(port_part) = after.strip_prefix(':') else {
            return Err(ReadinessError::InvalidPort);
        };
        let end = port_part
            .find('/')
            .or_else(|| port_part.find('?'))
            .or_else(|| port_part.find('#'))
            .unwrap_or(port_part.len());
        let port_str = &port_part[..end];
        let suffix = &port_part[end..];
        if port_str.is_empty() {
            return Err(ReadinessError::InvalidPort);
        }
        let host = IpAddr::V4(Ipv4Addr::LOCALHOST);
        (host, port_str, suffix)
    };

    if port_str.bytes().any(|b| !b.is_ascii_digit()) {
        return Err(ReadinessError::InvalidPort);
    }
    let port: u16 = port_str.parse().map_err(|_| ReadinessError::InvalidPort)?;
    if port == 0 {
        return Err(ReadinessError::InvalidPort);
    }

    // Suffix already checked for credentials via global '@' check; path is
    // preserved verbatim and not rewritten.

    Ok(ValidatedEndpoint {
        raw_url: url.to_owned(),
        host,
        port,
    })
}

/// Validates a single readiness line's bytes after `cap + 1` bounding.
///
/// The slice must be the exact bytes up to but not including the newline,
/// with one optional trailing `\r` already trimmed by the caller.
/// Uses bounded `serde_json::from_slice` per section 5: the byte slice is
/// already capped via `cap + 1`, then decoded as JSON. Requires exactly one
/// key named `url` whose value is a nonempty string.
fn parse_line_bytes(line: &[u8]) -> Result<ValidatedEndpoint, ReadinessError> {
    if line.is_empty() {
        return Err(ReadinessError::EmptyLine);
    }
    std::str::from_utf8(line).map_err(|_| ReadinessError::InvalidUtf8)?;
    let value: serde_json::Value =
        serde_json::from_slice(line).map_err(|_| ReadinessError::InvalidJson)?;
    let object = value.as_object().ok_or(ReadinessError::UnexpectedShape)?;
    if object.len() != 1 {
        return Err(ReadinessError::UnexpectedShape);
    }
    let url_value = object.get("url").ok_or(ReadinessError::UnexpectedShape)?;
    let url_str = url_value.as_str().ok_or(ReadinessError::UnexpectedShape)?;
    if url_str.is_empty() {
        return Err(ReadinessError::EmptyUrl);
    }
    validate_url(url_str)
}

/// Reads exactly one newline-terminated readiness record from the owned pipe.
///
/// `max_line` is the caller-supplied `max_readiness_line`; allocation is
/// bounded by checked `max_line + 1`. One optional trailing `\r` is trimmed
/// before validation. Failures are typed and payload-free.
///
/// `deadline` is the absolute `Instant` for the readiness phase. The call
/// also respects `shutdown` and `control` cancellation.
///
/// # Errors
///
/// Returns `ReadinessError::UnrepresentableCap` if `max_line + 1` overflows,
/// `ReadinessError::Oversized` if the line exceeds `max_line`,
/// `ReadinessError::TrailingBytes` if bytes follow the first newline,
/// `ReadinessError::EmptyLine` for an empty line, `ReadinessError::EofBeforeNewline`
/// if EOF precedes newline, `ReadinessError::InvalidUtf8`/`InvalidJson`/
/// `UnexpectedShape`/`EmptyUrl`/`InvalidScheme`/`InvalidHost`/`InvalidPort`/
/// `CredentialsPresent` for payload validation, or `Deadline`/`Cancelled`/
/// `Shutdown`/`Io` for transport/cancellation failures.
pub async fn read_readiness(
    stdout: &mut ChildStdout,
    max_line: usize,
    deadline: Instant,
    shutdown: &CancelHandle,
    control: &CancelHandle,
) -> Result<ValidatedEndpoint, ReadinessError> {
    let limit = max_line
        .checked_add(1)
        .ok_or(ReadinessError::UnrepresentableCap)?;
    let mut buf: Vec<u8> = Vec::new();
    // Reserve at most limit without over-allocating on huge limits already
    // validated by config; Vec will grow bounded by limit checks.
    let mut tmp = [0_u8; 512];

    loop {
        if shutdown.is_cancelled() {
            return Err(ReadinessError::Shutdown);
        }
        if control.is_cancelled() {
            return Err(ReadinessError::Cancelled);
        }
        if Instant::now() >= deadline {
            return Err(ReadinessError::Deadline);
        }

        // Search existing buffer for newline before reading.
        if let Some(pos) = buf.iter().position(|b| *b == b'\n') {
            return finish_buffer(&buf, pos);
        }

        if buf.len() >= limit {
            return Err(ReadinessError::Oversized);
        }

        let remaining = limit.saturating_sub(buf.len());
        let read_len = remaining.min(tmp.len());
        if read_len == 0 {
            return Err(ReadinessError::Oversized);
        }

        let read_result = tokio::select! {
            biased;
            () = shutdown.wait() => return Err(ReadinessError::Shutdown),
            () = control.wait() => return Err(ReadinessError::Cancelled),
            () = tokio::time::sleep_until(deadline) => return Err(ReadinessError::Deadline),
            res = stdout.read(&mut tmp[..read_len]) => res,
        };

        match read_result {
            Ok(0) => return Err(ReadinessError::EofBeforeNewline),
            Ok(n) => {
                buf.extend_from_slice(&tmp[..n]);
                if buf.len() > limit {
                    return Err(ReadinessError::Oversized);
                }
            }
            Err(_) => return Err(ReadinessError::Io),
        }
    }
}

fn finish_buffer(buf: &[u8], pos: usize) -> Result<ValidatedEndpoint, ReadinessError> {
    if buf.len() > pos + 1 {
        return Err(ReadinessError::TrailingBytes);
    }
    let mut line = &buf[..pos];
    if line.last() == Some(&b'\r') {
        line = &line[..line.len() - 1];
    }
    if line.is_empty() {
        return Err(ReadinessError::EmptyLine);
    }
    if line.len() > buf.len() {
        return Err(ReadinessError::Oversized);
    }
    parse_line_bytes(line)
}

/// Synchronous helper for direct line validation without pipe I/O.
///
/// `max_line` bounds `line` length after the same `cap + 1` rule. The input
/// must not include the trailing newline; one optional `\r` is trimmed.
///
/// # Errors
///
/// Returns `ReadinessError::UnrepresentableCap` on overflow, `ReadinessError::Oversized`
/// if the line exceeds `max_line`, `ReadinessError::EmptyLine` for empty input,
/// `ReadinessError::InvalidUtf8`/`InvalidJson`/`UnexpectedShape`/`EmptyUrl`/
/// `InvalidScheme`/`InvalidHost`/`InvalidPort`/`CredentialsPresent` for
/// content validation.
pub fn validate_readiness_line(
    line: &[u8],
    max_line: usize,
) -> Result<ValidatedEndpoint, ReadinessError> {
    let limit = max_line
        .checked_add(1)
        .ok_or(ReadinessError::UnrepresentableCap)?;
    if line.len() >= limit {
        return Err(ReadinessError::Oversized);
    }
    // Trim optional CR already part of line if caller included it; here we
    // assume newline already removed, but we still trim one CR if present.
    let mut trimmed = line;
    if trimmed.last() == Some(&b'\r') {
        trimmed = &trimmed[..trimmed.len() - 1];
    }
    if trimmed.is_empty() {
        return Err(ReadinessError::EmptyLine);
    }
    if trimmed.len() > max_line {
        return Err(ReadinessError::Oversized);
    }
    parse_line_bytes(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_ipv4() {
        let v = validate_url("http://127.0.0.1:1234").unwrap();
        assert_eq!(v.host(), IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!(v.port(), 1234);
    }

    #[test]
    fn valid_ipv6() {
        let v = validate_url("http://[::1]:8080").unwrap();
        assert_eq!(v.host(), IpAddr::V6(Ipv6Addr::LOCALHOST));
        assert_eq!(v.port(), 8080);
    }

    #[test]
    fn rejects_credentials() {
        let e = validate_url("http://user:pass@127.0.0.1:1234").unwrap_err();
        assert_eq!(e, ReadinessError::CredentialsPresent);
    }

    #[test]
    fn rejects_non_loopback() {
        let e = validate_url("http://192.168.1.1:1234").unwrap_err();
        assert_eq!(e, ReadinessError::InvalidHost);
    }

    #[test]
    fn rejects_zero_port() {
        let e = validate_url("http://127.0.0.1:0").unwrap_err();
        assert_eq!(e, ReadinessError::InvalidPort);
    }

    #[test]
    fn rejects_invalid_json() {
        let err = validate_readiness_line(b"not-json", 256).unwrap_err();
        assert_eq!(err, ReadinessError::InvalidJson);
    }

    #[test]
    fn rejects_unknown_shape_extra_key() {
        let err =
            validate_readiness_line(br#"{"url":"http://127.0.0.1:1","extra":1}"#, 256).unwrap_err();
        assert_eq!(err, ReadinessError::UnexpectedShape);
    }

    #[test]
    fn rejects_unknown_shape_wrong_type() {
        let err = validate_readiness_line(br#"{"url":123}"#, 256).unwrap_err();
        assert_eq!(err, ReadinessError::UnexpectedShape);
    }

    #[test]
    fn rejects_empty_url_via_shape() {
        let err = validate_readiness_line(br#"{"url":""}"#, 256).unwrap_err();
        assert_eq!(err, ReadinessError::EmptyUrl);
    }

    #[test]
    fn trims_optional_cr_before_json() {
        let ep = validate_readiness_line(b"{\"url\":\"http://127.0.0.1:9\"}\r", 256).unwrap();
        assert_eq!(ep.port(), 9);
    }
}
