//! Provider-account usage vocabulary for the native engine-usage query.
//!
//! This mirrors `EngineUsageQuery`, `EngineUsageReport`, and
//! `EngineUsageSnapshot` from `modules/protocol/src/engine-usage.ts`: one
//! provider-reported quota window is a used percentage with an optional
//! ISO-8601 reset instant, one engine report carries authentication state,
//! an explicit quota surface, and at most 64 windows, and one snapshot
//! carries at most 16 engine reports with a validated fetch instant.
//!
//! An empty `windows` list never implies anything about the provider's quota
//! API; `quota_surface` carries that distinction explicitly. Provider
//! failures, bearer tokens, and raw provider payloads never enter these
//! types: failures arrive as Artisan-owned bounded reason strings, and every
//! percentage is clamped to `0..=100` at construction so no fake precision
//! crosses the protocol boundary.
//!
//! The domain owns no clock: instants arrive as validated ISO-8601 strings,
//! and [`iso_millis`] formats signed Unix epoch milliseconds without
//! allocating through a time crate.

use std::fmt;
use std::str::FromStr;

use thiserror::Error;

use crate::bounds::{
    DISPLAY_NAME_MAX_BYTES, ENGINE_USAGE_EMAIL_MAX_BYTES, ENGINE_USAGE_ENGINES_MAX,
    ENGINE_USAGE_REASON_MAX_BYTES, ENGINE_USAGE_WINDOWS_MAX_PER_ENGINE, IDENTIFIER_MAX_BYTES,
};
use crate::identifiers::IdentifierError;

mod iso_time;
mod readiness;

pub use iso_time::{iso_millis, utc_ymd, validate_iso_timestamp};
pub use readiness::{EngineReadiness, EngineReadinessVerdict};

/// Classifies one provider quota window by its billing cadence.
///
/// Mirrors `EngineUsageWindowKind`: `session` is the short rolling window
/// (for example five hours), `weekly` the seven-day pool, `monthly` the
/// billing-cycle pool, and `unknown` any bucket whose cadence the reader
/// could not classify.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum EngineUsageWindowKind {
    /// Short rolling window (for example the five-hour session pool).
    Session,
    /// Seven-day pool (for example the all-models weekly window).
    Weekly,
    /// Billing-cycle pool (for example the monthly included-usage window).
    Monthly,
    /// A bucket whose cadence the reader could not classify.
    Unknown,
}

impl EngineUsageWindowKind {
    /// Returns the stable wire string for this window kind.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Weekly => "weekly",
            Self::Monthly => "monthly",
            Self::Unknown => "unknown",
        }
    }
}

impl fmt::Display for EngineUsageWindowKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for EngineUsageWindowKind {
    type Err = EngineUsageError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "session" => Ok(Self::Session),
            "weekly" => Ok(Self::Weekly),
            "monthly" => Ok(Self::Monthly),
            "unknown" => Ok(Self::Unknown),
            _ => Err(EngineUsageError::UnknownWindowKind {
                value: value.to_owned(),
            }),
        }
    }
}

/// Reports whether the provider account behind an engine can be billed.
///
/// Mirrors `EngineUsageAuthentication`.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum EngineUsageAuthentication {
    /// The provider accepted the account read.
    Authenticated,
    /// The provider rejected the account read as signed-out or expired.
    Unauthenticated,
    /// The reader could not determine the account state.
    Unknown,
}

impl EngineUsageAuthentication {
    /// Returns the stable wire string for this authentication state.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Authenticated => "authenticated",
            Self::Unauthenticated => "unauthenticated",
            Self::Unknown => "unknown",
        }
    }
}

impl fmt::Display for EngineUsageAuthentication {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for EngineUsageAuthentication {
    type Err = EngineUsageError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "authenticated" => Ok(Self::Authenticated),
            "unauthenticated" => Ok(Self::Unauthenticated),
            "unknown" => Ok(Self::Unknown),
            _ => Err(EngineUsageError::UnknownAuthentication {
                value: value.to_owned(),
            }),
        }
    }
}

/// Carries the explicit quota-surface distinction for one engine report.
///
/// Clients must not infer this from an empty window list: `supported` means
/// the provider exposes a real quota API, `unsupported` means the adapter
/// verified its provider has no account-usage surface, and `unknown` means
/// the reader could not reach the provider to tell.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum QuotaSurface {
    /// The provider exposes a real quota API.
    Supported,
    /// The reader could not reach the provider to determine the surface.
    Unknown,
    /// The adapter verified its provider has no account-usage surface.
    Unsupported,
}

impl QuotaSurface {
    /// Returns the stable wire string for this quota surface.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Supported => "supported",
            Self::Unknown => "unknown",
            Self::Unsupported => "unsupported",
        }
    }
}

impl fmt::Display for QuotaSurface {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for QuotaSurface {
    type Err = EngineUsageError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "supported" => Ok(Self::Supported),
            "unknown" => Ok(Self::Unknown),
            "unsupported" => Ok(Self::Unsupported),
            _ => Err(EngineUsageError::UnknownQuotaSurface {
                value: value.to_owned(),
            }),
        }
    }
}

/// Validation failure for a provider-account usage value.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum EngineUsageError {
    /// An engine or window identity violated the shared identifier rule.
    #[error("invalid usage identity: {source}")]
    Identity {
        /// Underlying shared identifier failure.
        #[source]
        source: IdentifierError,
    },
    /// A display name, label, or email violated its bound.
    #[error("invalid usage text in {field}: {reason}")]
    Text {
        /// Field being validated.
        field: &'static str,
        /// Stable validation reason.
        reason: &'static str,
    },
    /// A percentage was not finite and could not be clamped honestly.
    #[error("usage percent is not finite")]
    NonFinitePercent,
    /// A window cadence carried a non-positive minute count.
    #[error("usage window minutes must be positive")]
    NonPositiveWindowMinutes,
    /// A reset or fetch instant was not a valid ISO-8601 timestamp.
    #[error("invalid usage timestamp in {field}: {reason}")]
    Timestamp {
        /// Field being validated.
        field: &'static str,
        /// Stable validation reason.
        reason: &'static str,
    },
    /// A window kind string matched no known billing cadence.
    #[error("unknown usage window kind {value:?}")]
    UnknownWindowKind {
        /// The rejected kind string.
        value: String,
    },
    /// An authentication string matched no known account state.
    #[error("unknown usage authentication {value:?}")]
    UnknownAuthentication {
        /// The rejected authentication string.
        value: String,
    },
    /// A quota-surface string matched no known surface.
    #[error("unknown quota surface {value:?}")]
    UnknownQuotaSurface {
        /// The rejected surface string.
        value: String,
    },
    /// One engine report carried more than its bounded window list.
    #[error("usage report holds {count} windows; the maximum is {maximum}")]
    TooManyWindows {
        /// Offending window count.
        count: usize,
        /// Documented ceiling.
        maximum: usize,
    },
    /// One snapshot carried more than its bounded engine list.
    #[error("usage snapshot holds {count} engines; the maximum is {maximum}")]
    TooManyEngines {
        /// Offending engine count.
        count: usize,
        /// Documented ceiling.
        maximum: usize,
    },
}

impl From<IdentifierError> for EngineUsageError {
    fn from(source: IdentifierError) -> Self {
        Self::Identity { source }
    }
}

fn validate_usage_id(value: &str) -> Result<(), EngineUsageError> {
    if value.is_empty() {
        return Err(IdentifierError::Empty.into());
    }
    if let Some(character) = value
        .chars()
        .find(|character| character.is_whitespace() || character.is_control())
    {
        return Err(IdentifierError::ForbiddenCharacter { character }.into());
    }
    let length = value.len();
    if length > IDENTIFIER_MAX_BYTES {
        return Err(IdentifierError::TooLong {
            length,
            maximum: IDENTIFIER_MAX_BYTES,
        }
        .into());
    }
    Ok(())
}

fn validate_usage_text(
    value: &str,
    field: &'static str,
    maximum: usize,
) -> Result<(), EngineUsageError> {
    if value.is_empty() {
        return Err(EngineUsageError::Text {
            field,
            reason: "value must not be empty",
        });
    }
    if value.chars().any(char::is_control) {
        return Err(EngineUsageError::Text {
            field,
            reason: "value must not contain control characters",
        });
    }
    let length = value.len();
    if length > maximum {
        return Err(EngineUsageError::Text {
            field,
            reason: "value exceeds its UTF-8 byte ceiling",
        });
    }
    Ok(())
}

/// Clamps one finite provider-reported percentage to `0..=100`.
///
/// # Errors
///
/// Returns [`EngineUsageError::NonFinitePercent`] for NaN or infinite input
/// rather than inventing a replacement value.
pub fn clamp_percent_used(percent: f64) -> Result<f64, EngineUsageError> {
    if !percent.is_finite() {
        return Err(EngineUsageError::NonFinitePercent);
    }
    Ok(percent.clamp(0.0, 100.0))
}

/// One provider-reported quota window as a used percentage.
///
/// `id` is the provider's stable bucket identifier (for example `five_hour`
/// or `codex:model:primary`). `label` carries the provider's human bucket
/// name when one exists, so clients render provider vocabulary instead of
/// inventing their own.
#[derive(Clone, Debug, PartialEq)]
pub struct EngineUsageWindow {
    id: String,
    kind: EngineUsageWindowKind,
    label: Option<String>,
    percent_used: f64,
    resets_at: Option<String>,
    window_minutes: Option<u32>,
}

impl EngineUsageWindow {
    /// Creates one validated quota window, clamping a finite percentage.
    ///
    /// # Errors
    ///
    /// Returns [`EngineUsageError`] for an invalid id, label, non-finite
    /// percent, invalid reset timestamp, or non-positive window cadence.
    pub fn new(
        id: impl Into<String>,
        kind: EngineUsageWindowKind,
        label: Option<String>,
        percent_used: f64,
        resets_at: Option<String>,
        window_minutes: Option<u32>,
    ) -> Result<Self, EngineUsageError> {
        let id = id.into();
        validate_usage_id(&id)?;
        if let Some(label) = label.as_deref() {
            validate_usage_text(label, "label", DISPLAY_NAME_MAX_BYTES)?;
        }
        let percent_used = clamp_percent_used(percent_used)?;
        if let Some(resets_at) = resets_at.as_deref() {
            validate_iso_timestamp(resets_at, "resets_at")?;
        }
        if let Some(minutes) = window_minutes
            && minutes == 0
        {
            return Err(EngineUsageError::NonPositiveWindowMinutes);
        }
        Ok(Self {
            id,
            kind,
            label,
            percent_used,
            resets_at,
            window_minutes,
        })
    }

    /// Returns the provider's stable bucket identifier.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the billing-cadence classification.
    #[must_use]
    pub const fn kind(&self) -> EngineUsageWindowKind {
        self.kind
    }

    /// Returns the provider's human bucket name, when one exists.
    #[must_use]
    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }

    /// Returns the clamped used percentage in `0..=100`.
    #[must_use]
    pub const fn percent_used(&self) -> f64 {
        self.percent_used
    }

    /// Returns the validated ISO-8601 reset instant, when one is known.
    #[must_use]
    pub fn resets_at(&self) -> Option<&str> {
        self.resets_at.as_deref()
    }

    /// Returns the provider-reported window cadence in minutes, when known.
    #[must_use]
    pub const fn window_minutes(&self) -> Option<u32> {
        self.window_minutes
    }
}

// `Eq` is implemented manually rather than derived: `percent_used` is an
// `f64`, and `Eq` needs reflexivity. The constructor rejects NaN and
// infinite input and clamps finite values to `0..=100`, so every
// constructible window is reflexive and this impl is sound.
impl Eq for EngineUsageWindow {}

/// Provider-account authentication state with an optional bounded reason.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EngineUsageAuth {
    state: EngineUsageAuthentication,
    reason: Option<String>,
}

impl EngineUsageAuth {
    /// Creates one authentication state with an optional validated reason.
    ///
    /// # Errors
    ///
    /// Returns [`EngineUsageError`] when the reason is empty, carries control
    /// characters, or exceeds its UTF-8 byte ceiling.
    pub fn new(
        state: EngineUsageAuthentication,
        reason: Option<String>,
    ) -> Result<Self, EngineUsageError> {
        if let Some(reason) = reason.as_deref() {
            validate_usage_text(reason, "auth_reason", ENGINE_USAGE_REASON_MAX_BYTES)?;
        }
        Ok(Self { state, reason })
    }

    /// Returns the account state.
    #[must_use]
    pub const fn state(&self) -> EngineUsageAuthentication {
        self.state
    }

    /// Returns the bounded human reason, when one was supplied.
    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }
}

/// One engine's provider-account usage report.
///
/// An empty `windows` list does not imply that the provider lacks a quota
/// API; `quota_surface` carries that distinction explicitly.
#[derive(Clone, Debug, PartialEq)]
pub struct EngineUsageReport {
    account_email: Option<String>,
    authentication: EngineUsageAuth,
    display_name: String,
    engine_id: String,
    failure: Option<String>,
    quota_surface: Option<QuotaSurface>,
    windows: Vec<EngineUsageWindow>,
    readiness: EngineReadiness,
}

impl EngineUsageReport {
    /// Creates one validated engine usage report.
    ///
    /// # Errors
    ///
    /// Returns [`EngineUsageError`] for an invalid engine id, display name,
    /// email, reason, failure, timestamp-free fields, or more than
    /// [`ENGINE_USAGE_WINDOWS_MAX_PER_ENGINE`] windows.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        account_email: Option<String>,
        authentication: EngineUsageAuth,
        display_name: impl Into<String>,
        engine_id: impl Into<String>,
        failure: Option<String>,
        quota_surface: Option<QuotaSurface>,
        windows: Vec<EngineUsageWindow>,
    ) -> Result<Self, EngineUsageError> {
        let display_name = display_name.into();
        validate_usage_text(&display_name, "display_name", DISPLAY_NAME_MAX_BYTES)?;
        let engine_id = engine_id.into();
        validate_usage_id(&engine_id)?;
        if let Some(email) = account_email.as_deref() {
            if email.chars().any(char::is_control) {
                return Err(EngineUsageError::Text {
                    field: "account_email",
                    reason: "value must not contain control characters",
                });
            }
            validate_usage_text(email, "account_email", ENGINE_USAGE_EMAIL_MAX_BYTES)?;
        }
        if let Some(failure) = failure.as_deref() {
            validate_usage_text(failure, "failure", ENGINE_USAGE_REASON_MAX_BYTES)?;
        }
        let count = windows.len();
        if count > ENGINE_USAGE_WINDOWS_MAX_PER_ENGINE {
            return Err(EngineUsageError::TooManyWindows {
                count,
                maximum: ENGINE_USAGE_WINDOWS_MAX_PER_ENGINE,
            });
        }
        Ok(Self {
            account_email,
            authentication,
            display_name,
            engine_id,
            failure,
            quota_surface,
            windows,
            readiness: EngineReadiness::not_ready(),
        })
    }

    /// Returns the Forge's readiness verdict for this engine's account.
    /// A report the Forge has not judged reads as not ready.
    #[must_use]
    pub const fn readiness(&self) -> &EngineReadiness {
        &self.readiness
    }

    /// Returns this report carrying the Forge's readiness verdict.
    #[must_use]
    pub fn with_readiness(mut self, readiness: EngineReadiness) -> Self {
        self.readiness = readiness;
        self
    }

    /// Returns the provider account email, when the transport discloses one.
    #[must_use]
    pub fn account_email(&self) -> Option<&str> {
        self.account_email.as_deref()
    }

    /// Returns the provider-account authentication state.
    #[must_use]
    pub const fn authentication(&self) -> &EngineUsageAuth {
        &self.authentication
    }

    /// Returns the engine display name.
    #[must_use]
    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    /// Returns the stable engine id.
    #[must_use]
    pub fn engine_id(&self) -> &str {
        &self.engine_id
    }

    /// Returns the Artisan-owned failure reason, when the read failed.
    #[must_use]
    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    /// Returns the explicit quota-surface distinction, when known.
    #[must_use]
    pub const fn quota_surface(&self) -> Option<QuotaSurface> {
        self.quota_surface
    }

    /// Returns the bounded quota windows for this engine.
    #[must_use]
    pub fn windows(&self) -> &[EngineUsageWindow] {
        &self.windows
    }

    /// Returns a copy of this report marked with a refresh failure.
    ///
    /// Used when a refresh fails but a last-good reading exists: the served
    /// copy exposes the failure honestly while keeping the original windows
    /// and authentication. The cached original is never mutated.
    ///
    /// # Errors
    ///
    /// Returns [`EngineUsageError`] when the reason violates its bound.
    pub fn with_failure(&self, failure: String) -> Result<Self, EngineUsageError> {
        validate_usage_text(&failure, "failure", ENGINE_USAGE_REASON_MAX_BYTES)?;
        Ok(Self {
            failure: Some(failure),
            ..self.clone()
        })
    }
}

// Sound by the same constructor invariant as [`EngineUsageWindow`]: every
// contained window is reflexive, and all other fields already implement
// `Eq`.
impl Eq for EngineUsageReport {}

/// Requests provider-account usage.
///
/// `engine_id` narrows the read to one engine so clients can fan out per
/// engine and paint each report as it lands instead of waiting on the slowest
/// provider; absent, every registered engine reports. `force` marks a
/// user-initiated refresh: the backend re-asks providers even when its cached
/// reports are still inside the freshness window.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ReadAccountUsage {
    engine_id: Option<String>,
    force: bool,
}

impl ReadAccountUsage {
    /// Creates one validated account-usage query.
    ///
    /// # Errors
    ///
    /// Returns [`EngineUsageError`] when the engine id violates the shared
    /// identifier rule.
    pub fn new(engine_id: Option<String>, force: bool) -> Result<Self, EngineUsageError> {
        if let Some(engine_id) = engine_id.as_deref() {
            validate_usage_id(engine_id)?;
        }
        Ok(Self { engine_id, force })
    }

    /// Returns the narrowed engine id, when the client fanned out per engine.
    #[must_use]
    pub fn engine_id(&self) -> Option<&str> {
        self.engine_id.as_deref()
    }

    /// Returns whether cached reports must be refreshed from providers.
    #[must_use]
    pub const fn force(&self) -> bool {
        self.force
    }
}

/// Carries the provider-account usage reports for all registered engines.
#[derive(Clone, Debug, PartialEq)]
pub struct EngineUsageSnapshot {
    engines: Vec<EngineUsageReport>,
    fetched_at: String,
}

impl EngineUsageSnapshot {
    /// Creates one validated usage snapshot.
    ///
    /// # Errors
    ///
    /// Returns [`EngineUsageError`] for an invalid fetch instant or more than
    /// [`ENGINE_USAGE_ENGINES_MAX`] engine reports.
    pub fn new(
        engines: Vec<EngineUsageReport>,
        fetched_at: impl Into<String>,
    ) -> Result<Self, EngineUsageError> {
        let fetched_at = fetched_at.into();
        validate_iso_timestamp(&fetched_at, "fetched_at")?;
        let count = engines.len();
        if count > ENGINE_USAGE_ENGINES_MAX {
            return Err(EngineUsageError::TooManyEngines {
                count,
                maximum: ENGINE_USAGE_ENGINES_MAX,
            });
        }
        Ok(Self {
            engines,
            fetched_at,
        })
    }

    /// Returns the bounded per-engine reports.
    #[must_use]
    pub fn engines(&self) -> &[EngineUsageReport] {
        &self.engines
    }

    /// Returns the validated ISO-8601 fetch instant.
    #[must_use]
    pub fn fetched_at(&self) -> &str {
        &self.fetched_at
    }
}

// Sound by the same constructor invariant as [`EngineUsageWindow`]: every
// contained report is reflexive, and the fetch instant is a `String`.
impl Eq for EngineUsageSnapshot {}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(percent: f64) -> EngineUsageWindow {
        EngineUsageWindow::new(
            "five_hour",
            EngineUsageWindowKind::Session,
            None,
            percent,
            None,
            Some(300),
        )
        .expect("fixture window should validate")
    }

    #[expect(
        clippy::float_cmp,
        reason = "clamp bounds and pass-through must be bit-exact"
    )]
    #[test]
    fn percentages_clamp_while_non_finite_values_are_rejected() {
        assert_eq!(window(142.5).percent_used(), 100.0);
        assert_eq!(window(-3.0).percent_used(), 0.0);
        assert_eq!(window(42.25).percent_used(), 42.25);
        assert_eq!(
            EngineUsageWindow::new(
                "five_hour",
                EngineUsageWindowKind::Session,
                None,
                f64::NAN,
                None,
                Some(300),
            ),
            Err(EngineUsageError::NonFinitePercent)
        );
        assert_eq!(
            EngineUsageWindow::new(
                "five_hour",
                EngineUsageWindowKind::Session,
                None,
                f64::INFINITY,
                None,
                Some(300),
            ),
            Err(EngineUsageError::NonFinitePercent)
        );
    }

    #[test]
    fn window_identities_kinds_and_resets_validate() {
        assert!(
            EngineUsageWindow::new("", EngineUsageWindowKind::Session, None, 1.0, None, None)
                .is_err()
        );
        assert!(
            EngineUsageWindow::new(
                "has space",
                EngineUsageWindowKind::Session,
                None,
                1.0,
                None,
                None,
            )
            .is_err()
        );
        assert_eq!(
            EngineUsageWindow::new(
                "seven_day",
                EngineUsageWindowKind::Weekly,
                Some("Fable".to_owned()),
                10.0,
                Some("2026-09-09T12:00:00Z".to_owned()),
                Some(10_080),
            )
            .expect("weekly window should validate")
            .kind(),
            EngineUsageWindowKind::Weekly
        );
        assert_eq!(
            EngineUsageWindow::new(
                "seven_day",
                EngineUsageWindowKind::Weekly,
                None,
                10.0,
                Some("not-a-timestamp".to_owned()),
                None,
            ),
            Err(EngineUsageError::Timestamp {
                field: "resets_at",
                reason: "timestamp is shorter than YYYY-MM-DDTHH:MM:SSZ",
            })
        );
        assert_eq!(
            EngineUsageWindow::new(
                "seven_day",
                EngineUsageWindowKind::Weekly,
                None,
                10.0,
                None,
                Some(0),
            ),
            Err(EngineUsageError::NonPositiveWindowMinutes)
        );
        assert_eq!(
            EngineUsageWindowKind::from_str("monthly"),
            Ok(EngineUsageWindowKind::Monthly)
        );
        assert!(EngineUsageWindowKind::from_str("fortnightly").is_err());
    }

    #[test]
    fn iso_timestamps_enforce_shape_ranges_and_calendar() {
        for valid in [
            "2026-09-09T12:00:00Z",
            "2026-09-09T12:00:00.123Z",
            "2024-02-29T00:00:00Z",
            "1970-01-01T00:00:00+02:00",
            "2026-12-31T23:59:59.123456789-05:30",
        ] {
            validate_iso_timestamp(valid, "resets_at").expect("fixture timestamp should validate");
        }
        for invalid in [
            "",
            "2026-09-09",
            "2026-09-09T25:00:00Z",
            "2026-13-01T00:00:00Z",
            "2023-02-29T00:00:00Z",
            "2026-04-31T00:00:00Z",
            "2026-09-09T12:00:60Z",
            "2026-09-09 12:00:00Z",
            "2026-09-09T12:00:00",
            "2026-09-09T12:00:00+25:00",
            "26-09-09T12:00:00Z",
        ] {
            assert!(
                validate_iso_timestamp(invalid, "resets_at").is_err(),
                "{invalid:?} should not validate"
            );
        }
    }

    #[test]
    fn millis_format_round_trips_through_the_validator() {
        for millis in [0, 42, 1_788_955_200_000, -1, -86_400_000, 1_000] {
            let formatted = iso_millis(millis);
            validate_iso_timestamp(&formatted, "fetched_at")
                .expect("formatted millis should validate");
        }
        assert_eq!(iso_millis(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_millis(1_788_955_200_123), "2026-09-09T12:00:00.123Z");
        assert_eq!(iso_millis(-1), "1969-12-31T23:59:59.999Z");
    }

    #[test]
    fn reports_and_snapshots_enforce_collection_bounds() {
        let auth = EngineUsageAuth::new(EngineUsageAuthentication::Authenticated, None)
            .expect("auth should validate");
        let report = EngineUsageReport::new(
            Some("owner@example.test".to_owned()),
            auth,
            "Codex",
            "codex",
            None,
            Some(QuotaSurface::Supported),
            vec![window(10.0)],
        )
        .expect("report should validate");
        assert_eq!(report.engine_id(), "codex");
        assert_eq!(report.quota_surface(), Some(QuotaSurface::Supported));

        let windows = vec![window(1.0); ENGINE_USAGE_WINDOWS_MAX_PER_ENGINE + 1];
        assert_eq!(
            EngineUsageReport::new(
                None,
                EngineUsageAuth::new(EngineUsageAuthentication::Unknown, None)
                    .expect("auth should validate"),
                "Codex",
                "codex",
                None,
                None,
                windows,
            ),
            Err(EngineUsageError::TooManyWindows {
                count: ENGINE_USAGE_WINDOWS_MAX_PER_ENGINE + 1,
                maximum: ENGINE_USAGE_WINDOWS_MAX_PER_ENGINE,
            })
        );
        let engines = vec![
            EngineUsageReport::new(
                None,
                EngineUsageAuth::new(EngineUsageAuthentication::Unknown, None)
                    .expect("auth should validate"),
                "Codex",
                "codex",
                None,
                None,
                Vec::new(),
            )
            .expect("report should validate");
            ENGINE_USAGE_ENGINES_MAX + 1
        ];
        assert_eq!(
            EngineUsageSnapshot::new(engines, "2026-09-09T12:00:00Z"),
            Err(EngineUsageError::TooManyEngines {
                count: ENGINE_USAGE_ENGINES_MAX + 1,
                maximum: ENGINE_USAGE_ENGINES_MAX,
            })
        );
        assert!(
            EngineUsageSnapshot::new(Vec::new(), "2026-09-09T12:00:00Z")
                .expect("empty snapshot should validate")
                .engines()
                .is_empty()
        );
    }

    #[test]
    fn queries_validate_the_optional_engine_narrowing() {
        let query =
            ReadAccountUsage::new(Some("codex".to_owned()), true).expect("query should validate");
        assert_eq!(query.engine_id(), Some("codex"));
        assert!(query.force());
        let query = ReadAccountUsage::new(None, false).expect("query should validate");
        assert_eq!(query.engine_id(), None);
        assert!(!query.force());
        assert!(ReadAccountUsage::new(Some(String::new()), false).is_err());
        assert!(ReadAccountUsage::new(Some("has space".to_owned()), false).is_err());
    }
}
