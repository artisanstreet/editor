//! Bounded non-billable Cursor dashboard usage read.
//!
//! Mirrors `MakeCursorUsage` in `modules/engines/src/cursor/usage.ts`: read
//! the stored access token strictly read-only (bounded file read, never
//! written, refreshed, or logged in), POST an empty body to the
//! `DashboardService/GetCurrentPeriodUsage` endpoint through workspace
//! `hyper`, and map the billing-period pools to monthly quota windows. A
//! missing or expired token reports unauthenticated with an Artisan-owned
//! reason; HTTP 401/403 does the same. No token, header value, or response
//! payload is retained in a result or an error.
//!
//! Transport note: the production dashboard endpoint is HTTPS, and this
//! workspace declares no TLS connector crate for `hyper`. The POST exchange
//! therefore serves `http` endpoints (the fixture servers that prove it)
//! and answers `https` endpoints with an explicit
//! [`CursorUsageError::TlsUnavailable`] instead of attempting a plaintext
//! handshake or weakening certificate verification. Wiring a TLS connector
//! is a recorded root decision, not silent fallback behavior.

use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use artisan_domain::{EngineUsageWindow, EngineUsageWindowKind, clamp_percent_used, iso_millis};
use artisan_native_engine::account_usage::ProviderUsage;
use bytes::Bytes;
use http::Request;
use http_body_util::{BodyExt, Full, Limited};
use hyper::client::conn::http1::Builder;
use hyper_util::rt::TokioIo;

/// Production Cursor dashboard host.
pub const CURSOR_USAGE_HOST: &str = "api2.cursor.sh";
/// Production Cursor dashboard port.
pub const CURSOR_USAGE_PORT: u16 = 443;
/// Production `GetCurrentPeriodUsage` path.
pub const CURSOR_USAGE_PATH: &str = "/aiserver.v1.DashboardService/GetCurrentPeriodUsage";
/// Default dashboard read deadline (10 seconds).
pub const CURSOR_USAGE_TIMEOUT: Duration = Duration::from_secs(10);
/// Default credential-file and response byte ceiling (1 MiB).
pub const CURSOR_USAGE_MAX_BYTES: usize = 1_048_576;

/// Artisan-owned reason when no Cursor token is stored.
const CURSOR_SIGN_IN_REASON: &str = "Sign in to Cursor from Settings.";
/// Artisan-owned reason when the stored token is rejected.
const CURSOR_EXPIRED_REASON: &str = "Cursor sign-in is no longer valid.";

/// Payload-free failure of one Cursor dashboard usage read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CursorUsageError {
    /// The credential file could not be read.
    TokenIo,
    /// The credential file exceeded its byte bound.
    TokenTooLarge,
    /// The credential file was not the expected JSON shape.
    TokenMalformed,
    /// An `https` endpoint needs a TLS connector this build does not own.
    TlsUnavailable,
    /// The dashboard deadline elapsed.
    Timeout,
    /// TCP connect or the HTTP/1 handshake failed.
    ConnectFailed,
    /// Sending the dashboard request failed.
    SendFailed,
    /// The dashboard answered with a non-success, non-auth status.
    HttpFailure,
    /// The dashboard body exceeded its byte bound or could not be read.
    BodyTooLarge,
    /// The dashboard body was not the expected JSON shape.
    BodyMalformed,
}

impl fmt::Display for CursorUsageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::TokenIo => "cursor credential file could not be read",
            Self::TokenTooLarge => "cursor credential file exceeds its size bound",
            Self::TokenMalformed => "cursor credential file is malformed",
            Self::TlsUnavailable => {
                "cursor dashboard needs an https connector this build does not own"
            }
            Self::Timeout => "cursor dashboard deadline elapsed",
            Self::ConnectFailed => "cursor dashboard connection failed",
            Self::SendFailed => "cursor dashboard request failed",
            Self::HttpFailure => "cursor dashboard returned an unsuccessful status",
            Self::BodyTooLarge => "cursor dashboard body exceeded its bound",
            Self::BodyMalformed => "cursor dashboard body was malformed",
        })
    }
}

impl std::error::Error for CursorUsageError {}

/// Dashboard endpoint with an explicit plaintext/TLS distinction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CursorEndpoint {
    host: String,
    port: u16,
    path: String,
    use_tls: bool,
}

impl CursorEndpoint {
    /// Returns the production dashboard endpoint.
    #[must_use]
    pub fn production() -> Self {
        Self {
            host: CURSOR_USAGE_HOST.to_owned(),
            port: CURSOR_USAGE_PORT,
            path: CURSOR_USAGE_PATH.to_owned(),
            use_tls: true,
        }
    }

    /// Returns a plaintext fixture endpoint on loopback.
    #[must_use]
    pub fn plaintext(host: &str, port: u16) -> Self {
        Self {
            host: host.to_owned(),
            port,
            path: CURSOR_USAGE_PATH.to_owned(),
            use_tls: false,
        }
    }

    /// Returns whether this endpoint requires TLS.
    #[must_use]
    pub const fn use_tls(&self) -> bool {
        self.use_tls
    }
}

/// Configures one authenticated Cursor dashboard usage read.
#[derive(Clone, Debug)]
pub struct CursorUsageConfig {
    /// Stored credential file; resolved per platform when absent.
    pub auth_file: Option<PathBuf>,
    /// Dashboard endpoint; production by default.
    pub endpoint: CursorEndpoint,
    /// Caps the whole token-read plus POST sequence.
    pub timeout: Duration,
    /// Credential-file and response byte ceiling.
    pub max_bytes: usize,
}

impl CursorUsageConfig {
    /// Creates a production read configuration.
    #[must_use]
    pub fn new() -> Self {
        Self {
            auth_file: None,
            endpoint: CursorEndpoint::production(),
            timeout: CURSOR_USAGE_TIMEOUT,
            max_bytes: CURSOR_USAGE_MAX_BYTES,
        }
    }
}

impl Default for CursorUsageConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// Resolves the stored Cursor credential file for this platform.
///
/// Mirrors `cursor_auth_file_path`: `%APPDATA%/Cursor/auth.json` on Windows,
/// `~/.cursor/auth.json` on macOS, and `$XDG_CONFIG_HOME/cursor/auth.json`
/// (else `~/.config/cursor/auth.json`) elsewhere.
#[must_use]
pub fn cursor_auth_file_default() -> PathBuf {
    #[cfg(windows)]
    {
        if let Ok(app_data) = std::env::var("APPDATA") {
            if !app_data.is_empty() {
                return PathBuf::from(app_data).join("Cursor").join("auth.json");
            }
        }
        home_dir()
            .join("AppData")
            .join("Roaming")
            .join("Cursor")
            .join("auth.json")
    }
    #[cfg(target_os = "macos")]
    {
        home_dir().join(".cursor").join("auth.json")
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let base = std::env::var("XDG_CONFIG_HOME")
            .ok()
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home_dir().join(".config"));
        base.join("cursor").join("auth.json")
    }
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// Reads the stored access token strictly read-only.
///
/// Returns `Ok(None)` when the file is absent or carries no non-empty
/// `accessToken`; the caller reports unauthenticated. The read is bounded
/// and the file is opened read-only: never written, refreshed, or logged in.
///
/// # Errors
///
/// Returns [`CursorUsageError`] for I/O failures other than absence, an
/// oversized file, or a non-object JSON shape.
pub fn read_cursor_access_token(
    path: &std::path::Path,
    max_bytes: usize,
) -> Result<Option<String>, CursorUsageError> {
    let contents = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(CursorUsageError::TokenIo),
    };
    if contents.len() > max_bytes {
        return Err(CursorUsageError::TokenTooLarge);
    }
    let value: serde_json::Value =
        serde_json::from_slice(&contents).map_err(|_| CursorUsageError::TokenMalformed)?;
    let credential = value.as_object().ok_or(CursorUsageError::TokenMalformed)?;
    match credential.get("accessToken") {
        Some(serde_json::Value::String(token)) if !token.is_empty() => Ok(Some(token.clone())),
        None => Ok(None),
        _ => Ok(None),
    }
}

fn optional_number(
    object: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<Option<f64>, CursorUsageError> {
    match object.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::Number(number)) => number
            .as_f64()
            .filter(|value| value.is_finite())
            .map(Some)
            .ok_or(CursorUsageError::BodyMalformed),
        Some(serde_json::Value::String(text)) => text
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
            .map(Some)
            .ok_or(CursorUsageError::BodyMalformed),
        Some(_) => Err(CursorUsageError::BodyMalformed),
    }
}

fn percentage_from_display_message(message: &serde_json::Value) -> Option<f64> {
    // Extracts the first `N%` token from provider display text. This
    // deliberately differs from the TypeScript fallback regex
    // (`/\b(\d+(?:\.\d+)?)%\b/`), whose trailing `\b` only matches when `%`
    // is followed by a word character and therefore misses ordinary copy
    // like `"used 73% of"`. The intent is to surface the displayed percent;
    // the leading boundary is kept so versions like `v273%` do not match.
    let text = message.as_str()?;
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let mut start = index;
            let mut saw_digit = false;
            let mut saw_dot = false;
            while start > 0 {
                let byte = bytes[start - 1];
                if byte.is_ascii_digit() {
                    saw_digit = true;
                    start -= 1;
                } else if byte == b'.' && !saw_dot {
                    saw_dot = true;
                    start -= 1;
                } else {
                    break;
                }
            }
            if saw_digit
                && (start == 0 || !bytes[start - 1].is_ascii_alphanumeric())
                && text[start..index].parse::<f64>().is_ok()
            {
                return text[start..index].parse().ok();
            }
        }
        index += 1;
    }
    None
}

fn iso_millis_of(millis: f64) -> Option<String> {
    if !millis.is_finite() || millis.abs() > 8_640_000_000_000_000.0 {
        return None;
    }
    #[allow(clippy::cast_possible_truncation)]
    Some(iso_millis(millis.round() as i64))
}

/// Maps one `GetCurrentPeriodUsage` response to monthly quota windows.
///
/// Protobuf JSON omits numeric zeroes, so absent numerics inside a validated
/// usage object read as zero. Pure and free of network I/O.
///
/// # Errors
///
/// Returns [`CursorUsageError::BodyMalformed`] for a non-object response, a
/// missing `planUsage` object, or mistyped numeric fields.
pub fn map_cursor_period_usage(
    response: &serde_json::Value,
) -> Result<Vec<EngineUsageWindow>, CursorUsageError> {
    let response = response
        .as_object()
        .ok_or(CursorUsageError::BodyMalformed)?;
    let plan = response
        .get("planUsage")
        .and_then(serde_json::Value::as_object)
        .ok_or(CursorUsageError::BodyMalformed)?;
    let start_ms = optional_number(response, "billingCycleStart")?;
    let end_ms = optional_number(response, "billingCycleEnd")?;
    let resets_at = end_ms.and_then(iso_millis_of);
    let window_minutes = match (start_ms, end_ms) {
        (Some(start), Some(end)) if end > start => {
            let minutes = ((end - start) / 60_000.0).round();
            if minutes >= 1.0 && minutes <= f64::from(u32::MAX) {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                Some(minutes as u32)
            } else {
                None
            }
        }
        _ => None,
    };
    let total_spend = optional_number(plan, "totalSpend")?.unwrap_or(0.0);
    let included_limit = optional_number(plan, "limit")?.unwrap_or(0.0);
    let provider_percent = optional_number(plan, "totalPercentUsed")?;
    let display_percent = response
        .get("displayMessage")
        .and_then(percentage_from_display_message);
    let plan_percent = if included_limit > 0.0 {
        total_spend / included_limit * 100.0
    } else {
        provider_percent.or(display_percent).unwrap_or(0.0)
    };
    let cursor_models = optional_number(plan, "autoPercentUsed")?;
    let other_models = optional_number(plan, "apiPercentUsed")?;
    let has_auto_buckets = response
        .get("autoBucketModels")
        .is_some_and(|value| value.is_array());
    let pools: Vec<(&str, &str, f64)> =
        if cursor_models.is_some() || other_models.is_some() || has_auto_buckets {
            vec![
                (
                    "cursor:cursor-models",
                    "Cursor models",
                    cursor_models.unwrap_or(0.0),
                ),
                (
                    "cursor:other-models",
                    "Other models",
                    other_models.unwrap_or(0.0),
                ),
            ]
        } else {
            vec![("cursor:included-usage", "Included usage", plan_percent)]
        };
    let mut windows = Vec::with_capacity(pools.len() + 1);
    for (id, label, percent) in pools {
        let percent = clamp_percent_used(percent).map_err(|_| CursorUsageError::BodyMalformed)?;
        windows.push(
            EngineUsageWindow::new(
                id.to_owned(),
                EngineUsageWindowKind::Monthly,
                Some(label.to_owned()),
                percent,
                resets_at.clone(),
                window_minutes,
            )
            .map_err(|_| CursorUsageError::BodyMalformed)?,
        );
    }
    if let Some(spend) = response
        .get("spendLimitUsage")
        .and_then(serde_json::Value::as_object)
    {
        for (limit_key, used_key, remaining_key) in [
            ("overallLimit", "overallUsed", "overallRemaining"),
            ("individualLimit", "individualUsed", "individualRemaining"),
            ("pooledLimit", "pooledUsed", "pooledRemaining"),
        ] {
            let limit = optional_number(spend, limit_key)?.unwrap_or(0.0);
            if limit <= 0.0 {
                continue;
            }
            let remaining = optional_number(spend, remaining_key)?;
            let used = match optional_number(spend, used_key)? {
                Some(used) => used,
                None => remaining.map_or(0.0, |remaining| (limit - remaining).max(0.0)),
            };
            let percent = clamp_percent_used(used / limit * 100.0)
                .map_err(|_| CursorUsageError::BodyMalformed)?;
            windows.push(
                EngineUsageWindow::new(
                    "cursor:on-demand".to_owned(),
                    EngineUsageWindowKind::Monthly,
                    Some("On-demand".to_owned()),
                    percent,
                    resets_at.clone(),
                    window_minutes,
                )
                .map_err(|_| CursorUsageError::BodyMalformed)?,
            );
            break;
        }
    }
    Ok(windows)
}

/// POSTs an empty dashboard body through workspace `hyper`.
///
/// Serves `http` endpoints end to end. `https` endpoints answer
/// [`CursorUsageError::TlsUnavailable`]: this build owns no TLS connector,
/// and neither plaintext fallback nor unverified TLS is attempted.
///
/// # Errors
///
/// Returns [`CursorUsageError`] for TLS-gated endpoints, connect/send
/// failures, deadlines, oversized bodies, or malformed JSON.
pub async fn post_cursor_period_usage(
    endpoint: &CursorEndpoint,
    token: &str,
    timeout: Duration,
    max_bytes: usize,
) -> Result<(u16, serde_json::Value), CursorUsageError> {
    if endpoint.use_tls {
        return Err(CursorUsageError::TlsUnavailable);
    }
    let deadline = tokio::time::Instant::now() + timeout;
    let address = format!("{}:{}", endpoint.host, endpoint.port);
    let stream = tokio::time::timeout_at(deadline, tokio::net::TcpStream::connect(address))
        .await
        .map_err(|_| CursorUsageError::Timeout)?
        .map_err(|_| CursorUsageError::ConnectFailed)?;
    let io = TokioIo::new(stream);
    let (mut sender, connection) =
        tokio::time::timeout_at(deadline, Builder::new().handshake::<_, Full<Bytes>>(io))
            .await
            .map_err(|_| CursorUsageError::Timeout)?
            .map_err(|_| CursorUsageError::ConnectFailed)?;
    let driver = tokio::spawn(connection);
    let outcome = post_once(&mut sender, endpoint, token, deadline, max_bytes).await;
    driver.abort();
    let _join_result = driver.await;
    outcome
}

async fn post_once(
    sender: &mut hyper::client::conn::http1::SendRequest<Full<Bytes>>,
    endpoint: &CursorEndpoint,
    token: &str,
    deadline: tokio::time::Instant,
    max_bytes: usize,
) -> Result<(u16, serde_json::Value), CursorUsageError> {
    let host_header = format!("{}:{}", endpoint.host, endpoint.port);
    let request = Request::builder()
        .method("POST")
        .uri(endpoint.path.clone())
        .header("host", host_header)
        .header("authorization", format!("Bearer {token}"))
        .header("connect-protocol-version", "1")
        .header("content-type", "application/json")
        .header("x-cursor-client-type", "cli")
        .header("content-length", "2")
        .header("connection", "close")
        .body(Full::new(Bytes::from_static(b"{}")))
        .map_err(|_| CursorUsageError::SendFailed)?;
    if tokio::time::Instant::now() >= deadline {
        return Err(CursorUsageError::Timeout);
    }
    sender
        .ready()
        .await
        .map_err(|_| CursorUsageError::SendFailed)?;
    let response = tokio::time::timeout_at(deadline, sender.send_request(request))
        .await
        .map_err(|_| CursorUsageError::Timeout)?
        .map_err(|_| CursorUsageError::SendFailed)?;
    let status = response.status().as_u16();
    let body = response.into_body();
    let collected = tokio::time::timeout_at(deadline, Limited::new(body, max_bytes).collect())
        .await
        .map_err(|_| CursorUsageError::Timeout)?
        .map_err(|_| CursorUsageError::BodyTooLarge)?;
    let bytes = collected.to_bytes();
    if bytes.len() > max_bytes {
        return Err(CursorUsageError::BodyTooLarge);
    }
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| CursorUsageError::BodyMalformed)?;
    Ok((status, value))
}

/// Reads Cursor billing-period usage without starting a model session.
///
/// The bearer token travels only to the configured dashboard endpoint and is
/// never included in a result or an error.
///
/// # Errors
///
/// Returns [`CursorUsageError`] for credential, transport, deadline, bound,
/// or mapping failures. HTTP 401/403 and a missing token instead yield an
/// unauthenticated [`ProviderUsage`].
pub async fn read_cursor_usage(
    config: &CursorUsageConfig,
) -> Result<ProviderUsage, CursorUsageError> {
    let auth_file = config
        .auth_file
        .clone()
        .unwrap_or_else(cursor_auth_file_default);
    let token = read_cursor_access_token(&auth_file, config.max_bytes)?;
    let Some(token) = token else {
        return Ok(ProviderUsage::unauthenticated(CURSOR_SIGN_IN_REASON));
    };
    let (status, payload) =
        post_cursor_period_usage(&config.endpoint, &token, config.timeout, config.max_bytes)
            .await?;
    if status == 401 || status == 403 {
        return Ok(ProviderUsage::unauthenticated(CURSOR_EXPIRED_REASON));
    }
    if !(200..300).contains(&status) {
        return Err(CursorUsageError::HttpFailure);
    }
    Ok(ProviderUsage::authenticated(map_cursor_period_usage(
        &payload,
    )?))
}
