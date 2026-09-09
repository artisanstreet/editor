//! Bounded non-billable Codex account-usage read.
//!
//! Mirrors `MakeCodexUsage` in `modules/engines/src/codex/usage.ts`: open the
//! `codex app-server --stdio` JSON-RPC surface, run the `initialize` /
//! `initialized` handshake, read `account/read` as the sole authentication
//! authority, then map `account/rateLimits/read` buckets to provider-neutral
//! quota windows. No credential file is read and no run is started. The
//! process is always killed and reaped by the session guard, and the complete
//! sequence is bounded by one overall deadline.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use artisan_domain::{
    EngineUsageWindow, EngineUsageWindowKind, QuotaSurface, clamp_percent_used, iso_millis,
};

use super::account_usage::{
    CallError, CliLaunch, ExchangeBounds, JsonRpcSession, ProviderUsage, UsageReaderError,
};

/// Default overall deadline for one Codex usage exchange (15 seconds).
pub const CODEX_USAGE_OVERALL_TIMEOUT: Duration = Duration::from_secs(15);

/// Notification methods the usage handshake opts out of, mirroring
/// `codex_opt_out_notification_methods`.
const CODEX_OPT_OUT_NOTIFICATION_METHODS: &[&str] = &[
    "account/rateLimits/updated",
    "mcpServer/startupStatus/updated",
    "remoteControl/status/changed",
];

/// Artisan-owned reason when no Codex account is active.
const CODEX_UNAUTHENTICATED_REASON: &str = "Codex account sign-in is required.";
/// Artisan-owned reason when Codex demands OpenAI authentication.
const CODEX_OPENAI_AUTH_REASON: &str = "OpenAI authentication is required.";

/// Configures one non-billable Codex account-usage read.
#[derive(Clone, Debug)]
pub struct CodexUsageConfig {
    /// Provider executable (absolute path or PATH-resolved name).
    pub executable: PathBuf,
    /// Extra arguments before `app-server --stdio`.
    pub executable_args: Vec<String>,
    /// Interpreter prefix placed between the program and `executable_args`
    /// (from [`CliLaunch`], empty for direct launches).
    pub prefix_args: Vec<String>,
    /// Extra environment for the spawned child (fixture seam).
    pub spawn_env: Vec<(String, String)>,
    /// Caps the whole spawn-handshake-request sequence.
    pub overall_timeout: Duration,
    /// Byte and frame bounds for the stdio exchange.
    pub bounds: ExchangeBounds,
}

impl CodexUsageConfig {
    /// Creates a read configuration with the documented default bounds.
    #[must_use]
    pub fn new(executable: PathBuf) -> Self {
        Self {
            executable,
            executable_args: Vec::new(),
            prefix_args: Vec::new(),
            spawn_env: Vec::new(),
            overall_timeout: CODEX_USAGE_OVERALL_TIMEOUT,
            bounds: ExchangeBounds::defaults(),
        }
    }

    /// Creates a read configuration from one resolved CLI launch.
    #[must_use]
    pub fn launched(launch: &CliLaunch) -> Self {
        let mut config = Self::new(launch.program.clone());
        config.prefix_args = launch.prefix_args.clone();
        config
    }
}

/// Reads Codex account usage without starting a run.
///
/// # Errors
///
/// Returns [`UsageReaderError`] for spawn, deadline, bound, framing, exit,
/// or malformed-payload failures. A login-gated provider error becomes an
/// unauthenticated [`ProviderUsage`], never a failure.
pub fn read_codex_usage(config: &CodexUsageConfig) -> Result<ProviderUsage, UsageReaderError> {
    let deadline = Instant::now() + config.overall_timeout;
    let mut args = config.prefix_args.clone();
    args.extend(config.executable_args.iter().cloned());
    args.push("app-server".to_owned());
    args.push("--stdio".to_owned());
    let mut session =
        JsonRpcSession::spawn(&config.executable, &args, &config.spawn_env, config.bounds)?;
    call_with_deadline(&mut session, deadline, "initialize", initialize_params())
        .map_err(into_reader_error)?;
    session
        .notify("initialized", serde_json::json!({}))
        .map_err(|_| UsageReaderError::Closed)?;
    let account = match call_with_deadline(
        &mut session,
        deadline,
        "account/read",
        serde_json::json!({}),
    ) {
        Ok(account) => account,
        Err(CallError::Provider(error)) if error.is_login_error() => {
            return Ok(ProviderUsage::unauthenticated(CODEX_UNAUTHENTICATED_REASON));
        }
        Err(error) => return Err(map_call_error(error, deadline)),
    };
    let email = match map_codex_account(&account)? {
        CodexAccount::Active { email } => email,
        CodexAccount::Inactive { openai_auth: true } => {
            return Ok(ProviderUsage::unauthenticated(CODEX_OPENAI_AUTH_REASON));
        }
        CodexAccount::Inactive { openai_auth: false } => {
            return Ok(ProviderUsage::unauthenticated(CODEX_UNAUTHENTICATED_REASON));
        }
    };
    let limits = match call_with_deadline(
        &mut session,
        deadline,
        "account/rateLimits/read",
        serde_json::json!({}),
    ) {
        Ok(limits) => limits,
        Err(CallError::Provider(error)) if error.is_login_error() => {
            return Ok(ProviderUsage::unauthenticated(CODEX_UNAUTHENTICATED_REASON));
        }
        Err(error) => return Err(map_call_error(error, deadline)),
    };
    let mut usage = ProviderUsage::authenticated(map_codex_rate_limits(&limits)?);
    usage.account_email = email;
    Ok(usage)
}

fn call_with_deadline(
    session: &mut JsonRpcSession,
    deadline: Instant,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, CallError> {
    if Instant::now() >= deadline {
        return Err(CallError::Transport(UsageReaderError::Timeout));
    }
    session.call(method, params, deadline)
}

fn into_reader_error(error: CallError) -> UsageReaderError {
    match error {
        CallError::Transport(error) => error,
        CallError::Provider(_) => UsageReaderError::Protocol,
    }
}

fn map_call_error(error: CallError, deadline: Instant) -> UsageReaderError {
    match error {
        CallError::Transport(UsageReaderError::Timeout) => UsageReaderError::Timeout,
        CallError::Transport(error) if Instant::now() >= deadline => {
            let _ = error;
            UsageReaderError::Timeout
        }
        CallError::Transport(error) => error,
        CallError::Provider(_) => UsageReaderError::Protocol,
    }
}

fn initialize_params() -> serde_json::Value {
    serde_json::json!({
        "capabilities": {
            "experimentalApi": false,
            "optOutNotificationMethods": CODEX_OPT_OUT_NOTIFICATION_METHODS,
            "requestAttestation": false,
        },
        "clientInfo": {
            "name": "artisan-usage",
            "version": env!("CARGO_PKG_VERSION"),
        },
    })
}

/// Classified Codex account state: an active account with an optional
/// disclosed email, or an inactive account with its OpenAI-auth distinction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CodexAccount {
    /// An active `apiKey`, `chatgpt`, or `amazonBedrock` account.
    Active {
        /// ChatGPT account email, when the transport discloses a valid one.
        email: Option<String>,
    },
    /// No ChatGPT or API-key account is active.
    Inactive {
        /// Whether Codex explicitly demands OpenAI authentication.
        openai_auth: bool,
    },
}

/// Maps one decoded `account/read` result to its classified account state.
///
/// # Errors
///
/// Returns [`UsageReaderError::Malformed`] for a non-object result, a
/// missing account shape, or an unknown account type.
pub fn map_codex_account(result: &serde_json::Value) -> Result<CodexAccount, UsageReaderError> {
    let object = result.as_object().ok_or(UsageReaderError::Malformed)?;
    let account = object.get("account").ok_or(UsageReaderError::Malformed)?;
    if account.is_null() {
        let openai_auth = object
            .get("requiresOpenaiAuth")
            .and_then(serde_json::Value::as_bool)
            .ok_or(UsageReaderError::Malformed)?;
        return Ok(CodexAccount::Inactive { openai_auth });
    }
    let account = account.as_object().ok_or(UsageReaderError::Malformed)?;
    let account_type = account
        .get("type")
        .and_then(serde_json::Value::as_str)
        .ok_or(UsageReaderError::Malformed)?;
    match account_type {
        "apiKey" | "amazonBedrock" => Ok(CodexAccount::Active { email: None }),
        "chatgpt" => {
            let email = match account.get("email") {
                None | Some(serde_json::Value::Null) => None,
                Some(serde_json::Value::String(email)) if email.is_empty() => None,
                Some(serde_json::Value::String(email)) => Some(email.clone()),
                Some(_) => return Err(UsageReaderError::Malformed),
            };
            Ok(CodexAccount::Active { email })
        }
        _ => Err(UsageReaderError::Malformed),
    }
}

fn codex_reset_at(resets_at: &serde_json::Value) -> Result<Option<String>, UsageReaderError> {
    match resets_at {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::Number(seconds) => {
            let Some(seconds) = seconds.as_f64() else {
                return Ok(None);
            };
            if !seconds.is_finite() {
                return Ok(None);
            }
            // Bound the instant far beyond any real provider reset so the
            // float-to-integer cast below cannot saturate into a fake time.
            if seconds.abs() > 8_640_000_000_000.0 {
                return Ok(None);
            }
            #[allow(clippy::cast_possible_truncation)]
            let millis = (seconds * 1_000.0).round() as i64;
            Ok(Some(iso_millis(millis)))
        }
        _ => Err(UsageReaderError::Malformed),
    }
}

fn classify_codex_window_kind(window_minutes: Option<u32>) -> EngineUsageWindowKind {
    match window_minutes {
        None => EngineUsageWindowKind::Unknown,
        Some(300) => EngineUsageWindowKind::Session,
        Some(10_080) => EngineUsageWindowKind::Weekly,
        Some(minutes) if (40_000..=45_000).contains(&minutes) => EngineUsageWindowKind::Monthly,
        Some(_) => EngineUsageWindowKind::Unknown,
    }
}

fn codex_window_minutes(value: &serde_json::Value) -> Result<Option<u32>, UsageReaderError> {
    match value {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::Number(minutes) => {
            let Some(minutes) = minutes.as_f64() else {
                return Err(UsageReaderError::Malformed);
            };
            if !minutes.is_finite() || minutes <= 0.0 {
                return Ok(None);
            }
            if minutes.fract() != 0.0 || minutes > f64::from(u32::MAX) {
                return Err(UsageReaderError::Malformed);
            }
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            Ok(Some(minutes as u32))
        }
        _ => Err(UsageReaderError::Malformed),
    }
}

fn codex_percent(value: &serde_json::Value) -> Result<f64, UsageReaderError> {
    match value {
        serde_json::Value::Null => Ok(0.0),
        serde_json::Value::Number(percent) => {
            let Some(percent) = percent.as_f64() else {
                return Err(UsageReaderError::Malformed);
            };
            clamp_percent_used(percent).map_err(|_| UsageReaderError::Malformed)
        }
        _ => Err(UsageReaderError::Malformed),
    }
}

/// Maps one decoded `account/rateLimits/read` result to quota windows.
///
/// Iterates `rateLimitsByLimitId` when present, otherwise falls back to the
/// single `rateLimits` snapshot, emitting one window per non-null
/// `primary`/`secondary` slot in bucket-then-slot order. Pure and free of
/// process I/O so it can be exercised without spawning Codex.
///
/// # Errors
///
/// Returns [`UsageReaderError::Malformed`] for a non-object result, a
/// non-object bucket map, or mistyped window fields.
pub fn map_codex_rate_limits(
    result: &serde_json::Value,
) -> Result<Vec<EngineUsageWindow>, UsageReaderError> {
    let object = result.as_object().ok_or(UsageReaderError::Malformed)?;
    let mut buckets: Vec<(String, &serde_json::Value)> = Vec::new();
    match object.get("rateLimitsByLimitId") {
        None | Some(serde_json::Value::Null) => {
            if let Some(snapshot) = object.get("rateLimits") {
                let fallback = snapshot
                    .get("limitId")
                    .and_then(serde_json::Value::as_str)
                    .filter(|id| !id.is_empty())
                    .unwrap_or("codex")
                    .to_owned();
                buckets.push((fallback, snapshot));
            }
        }
        Some(serde_json::Value::Object(map)) => {
            for (limit_id, snapshot) in map {
                buckets.push((limit_id.clone(), snapshot));
            }
        }
        Some(_) => return Err(UsageReaderError::Malformed),
    }
    let mut windows = Vec::new();
    for (bucket_id, snapshot) in &buckets {
        let snapshot = snapshot.as_object().ok_or(UsageReaderError::Malformed)?;
        let label = snapshot
            .get("limitName")
            .and_then(serde_json::Value::as_str)
            .filter(|name| !name.is_empty())
            .map(str::to_owned);
        for slot in ["primary", "secondary"] {
            let Some(window) = snapshot.get(slot) else {
                continue;
            };
            if window.is_null() {
                continue;
            }
            let window = window.as_object().ok_or(UsageReaderError::Malformed)?;
            let default_null = serde_json::Value::Null;
            let slot_value = window.get("usedPercent").unwrap_or(&default_null);
            let percent_used = codex_percent(slot_value)?;
            let resets_at = match window.get("resetsAt") {
                None => None,
                Some(value) => codex_reset_at(value)?,
            };
            let window_minutes = match window.get("windowDurationMins") {
                None => None,
                Some(value) => codex_window_minutes(value)?,
            };
            let id = format!("{bucket_id}:{slot}");
            windows.push(
                EngineUsageWindow::new(
                    id,
                    classify_codex_window_kind(window_minutes),
                    label.clone(),
                    percent_used,
                    resets_at,
                    window_minutes,
                )
                .map_err(|_| UsageReaderError::Malformed)?,
            );
        }
    }
    Ok(windows)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rate_limits_fixture() -> serde_json::Value {
        serde_json::json!({
            "rateLimitsByLimitId": {
                "codex": {
                    "limitId": "codex",
                    "limitName": null,
                    "primary": {"resetsAt": 1_788_955_200, "usedPercent": 42.5, "windowDurationMins": 300},
                    "secondary": {"resetsAt": 1_849_363_200, "usedPercent": 150.0, "windowDurationMins": 10_080}
                },
                "gpt-5": {
                    "limitId": "gpt-5",
                    "limitName": "Fable",
                    "primary": {"resetsAt": null, "usedPercent": -3.0, "windowDurationMins": 43_200},
                    "secondary": null
                },
                "weird": {
                    "limitId": "weird",
                    "limitName": "Weird",
                    "primary": {"usedPercent": 7.0, "windowDurationMins": 123},
                    "secondary": {"usedPercent": 8.0}
                }
            }
        })
    }

    #[test]
    fn bucket_mapping_clamps_classifies_and_keeps_provider_vocabulary() {
        let windows = map_codex_rate_limits(&rate_limits_fixture()).expect("fixture maps");
        assert_eq!(windows.len(), 5);
        assert_eq!(windows[0].id(), "codex:primary");
        assert_eq!(windows[0].kind(), EngineUsageWindowKind::Session);
        assert_eq!(windows[0].percent_used(), 42.5);
        assert_eq!(windows[0].resets_at(), Some("2026-09-09T12:00:00Z"));
        assert_eq!(windows[0].window_minutes(), Some(300));

        assert_eq!(windows[1].id(), "codex:secondary");
        assert_eq!(windows[1].kind(), EngineUsageWindowKind::Weekly);
        assert_eq!(windows[1].percent_used(), 100.0);

        assert_eq!(windows[2].id(), "gpt-5:primary");
        assert_eq!(windows[2].kind(), EngineUsageWindowKind::Monthly);
        assert_eq!(windows[2].label(), Some("Fable"));
        assert_eq!(windows[2].percent_used(), 0.0);
        assert_eq!(windows[2].resets_at(), None);

        assert_eq!(windows[3].id(), "weird:primary");
        assert_eq!(windows[3].kind(), EngineUsageWindowKind::Unknown);
        assert_eq!(windows[4].id(), "weird:secondary");
        assert_eq!(windows[4].kind(), EngineUsageWindowKind::Unknown);
        assert_eq!(windows[4].window_minutes(), None);
    }

    #[test]
    fn single_snapshot_fallback_and_malformed_payloads() {
        let single = serde_json::json!({
            "rateLimits": {"limitName": "Solo", "primary": {"usedPercent": 11.0}}
        });
        let windows = map_codex_rate_limits(&single).expect("single snapshot maps");
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].id(), "codex:primary");

        let empty = serde_json::json!({});
        assert!(
            map_codex_rate_limits(&empty)
                .expect("empty maps")
                .is_empty()
        );

        for malformed in [
            serde_json::json!([]),
            serde_json::json!({"rateLimitsByLimitId": []}),
            serde_json::json!({"rateLimitsByLimitId": {"x": []}}),
            serde_json::json!({"rateLimitsByLimitId": {"x": {"primary": {"usedPercent": "lots"}}}}),
            serde_json::json!({"rateLimitsByLimitId": {"x": {"primary": {"resetsAt": "soon"}}}}),
            serde_json::json!({"rateLimitsByLimitId": {"x": {"primary": {"windowDurationMins": 1.5}}}}),
        ] {
            assert_eq!(
                map_codex_rate_limits(&malformed),
                Err(UsageReaderError::Malformed),
                "payload should be rejected: {malformed}"
            );
        }
    }

    #[test]
    fn account_mapping_distinguishes_active_chatgpt_and_api_key() {
        let chatgpt = serde_json::json!({
            "account": {"type": "chatgpt", "email": "owner@example.test", "planType": "plus"},
            "requiresOpenaiAuth": false,
        });
        assert_eq!(
            map_codex_account(&chatgpt).expect("chatgpt maps"),
            CodexAccount::Active {
                email: Some("owner@example.test".to_owned())
            }
        );
        let key = serde_json::json!({"account": {"type": "apiKey"}, "requiresOpenaiAuth": false});
        assert_eq!(
            map_codex_account(&key).expect("api key maps"),
            CodexAccount::Active { email: None }
        );
        let missing = serde_json::json!({"account": null, "requiresOpenaiAuth": false});
        assert_eq!(
            map_codex_account(&missing).expect("missing maps"),
            CodexAccount::Inactive { openai_auth: false }
        );
        let openai = serde_json::json!({"account": null, "requiresOpenaiAuth": true});
        assert_eq!(
            map_codex_account(&openai).expect("openai maps"),
            CodexAccount::Inactive { openai_auth: true }
        );
        let unknown =
            serde_json::json!({"account": {"type": "oauth2"}, "requiresOpenaiAuth": false});
        assert_eq!(
            map_codex_account(&unknown),
            Err(UsageReaderError::Malformed)
        );
        assert_eq!(
            map_codex_account(&serde_json::json!({"account": null})),
            Err(UsageReaderError::Malformed)
        );
    }

    #[test]
    fn quota_surface_is_supported_for_codex_reads() {
        let usage = ProviderUsage::authenticated(Vec::new());
        assert_eq!(usage.quota_surface, QuotaSurface::Supported);
    }
}
