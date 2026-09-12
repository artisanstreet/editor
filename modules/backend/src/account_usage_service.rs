//! Per-engine provider-account usage fan-out with freshness caching.
//!
//! The service owns the fixed engine roster, one freshness window shared by
//! every engine, and concurrent bounded reads. `engine_id` narrowing fans
//! out per requested engine; absent, every registered engine reports in
//! roster order. `force` re-asks providers even when cached reports are
//! fresh. One provider's failure never fails its siblings: every engine
//! always yields exactly one report carrying an explicit failure or
//! authentication state with an honest quota surface, never an inert
//! unavailable-only response and never token run-usage.
//!
//! Freshness matches the Electron service (180 seconds). A failed refresh
//! preserves the last-good report with its original fetch time while
//! exposing the refresh failure honestly on the served report, so clients
//! never mistake stale data for a fresh check. Each cached report keeps its
//! own provider observation timestamp: a narrowed single-engine snapshot
//! carries exactly that engine's observation time, while an aggregate
//! snapshot carries the latest observation time across its reports.
//! Clients that need exact per-engine freshness issue one narrowed query
//! per engine.
//!
//! Engine coverage mirrors the TypeScript adapters: Codex reads
//! `account/rateLimits/read`, Claude parses `claude -p /usage`, Cursor posts
//! its dashboard endpoint, and Grok Build, Hermes, and `OpenCode` report
//! unsupported-with-reason because their adapters expose no account-usage
//! surface (`Engine.Usage` is optional in `modules/engines/src/engine.ts`
//! and absent from those three adapters).

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use artisan_domain::{
    EngineUsageAuth, EngineUsageAuthentication, EngineUsageReport, EngineUsageSnapshot,
    QuotaSurface, ReadAccountUsage, iso_millis,
};
use artisan_native_engine::account_usage::{ProviderUsage, UsageReaderError};
use tokio::task::JoinSet;

use super::account_usage_cursor::{CursorUsageConfig, CursorUsageError, read_cursor_usage};
use artisan_native_engine::{ClaudeUsageConfig, CliLaunch, CodexUsageConfig};

/// Freshness window for cached per-engine reports (180 seconds).
///
/// Matches the Electron usage service and frontend controller so Forge and
/// the legacy client agree on when a provider read goes stale.
pub const ACCOUNT_USAGE_FRESHNESS: Duration = Duration::from_secs(180);
/// Per-engine read deadline enforced around every reader (30 seconds).
///
/// Reader-internal deadlines are shorter (Codex 15s, Claude 20s, Cursor
/// 10s), so an abandoned service wait still leaves child custody to the
/// reader that owns it.
pub const ACCOUNT_USAGE_PER_ENGINE_TIMEOUT: Duration = Duration::from_secs(30);

const READ_TIMED_OUT: &str = "engine usage read timed out";
const READ_FAILED: &str = "engine usage read failed";
const UNREPRESENTABLE_READ: &str = "provider usage could not be represented";
const UNKNOWN_ENGINE: &str = "unknown engine id";
const GROK_UNSUPPORTED: &str = "Grok Build exposes no account-usage surface.";
const HERMES_UNSUPPORTED: &str = "Hermes exposes no account-usage surface.";
const OPENCODE2_UNSUPPORTED: &str = "OpenCode exposes no account-usage surface.";
const CODEX_DISPLAY: &str = "Codex";
const CLAUDE_DISPLAY: &str = "Claude";
const CURSOR_DISPLAY: &str = "Cursor";
const GROK_DISPLAY: &str = "Grok Build";
const HERMES_DISPLAY: &str = "Hermes";
const OPENCODE2_DISPLAY: &str = "OpenCode";

/// Typed failure of one engine read with its honest report states.
///
/// Successful reads never produce this value; every field maps directly
/// onto an [`EngineUsageReport`] with empty windows.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReaderFailure {
    /// Authentication state to report for the failed engine.
    pub auth_state: EngineUsageAuthentication,
    /// Artisan-owned authentication reason, when one applies.
    pub auth_reason: Option<String>,
    /// Honest quota surface for the failed engine.
    pub quota_surface: QuotaSurface,
    /// Artisan-owned failure reason; never a provider payload.
    pub failure: String,
}

impl ReaderFailure {
    /// Creates one typed engine-read failure.
    #[must_use]
    pub fn new(
        auth_state: EngineUsageAuthentication,
        auth_reason: Option<String>,
        quota_surface: QuotaSurface,
        failure: &'static str,
    ) -> Self {
        Self {
            auth_state,
            auth_reason,
            quota_surface,
            failure: failure.to_owned(),
        }
    }

    /// Creates an unreachable-provider failure.
    #[must_use]
    pub fn unavailable(failure: &'static str) -> Self {
        Self::new(
            EngineUsageAuthentication::Unknown,
            None,
            QuotaSurface::Unknown,
            failure,
        )
    }

    /// Creates a malformed-provider-response failure.
    #[must_use]
    pub fn protocol(failure: &'static str) -> Self {
        Self::new(
            EngineUsageAuthentication::Unknown,
            None,
            QuotaSurface::Unknown,
            failure,
        )
    }

    /// Creates an adapter-without-account-surface failure.
    #[must_use]
    pub fn unsupported(failure: &'static str) -> Self {
        Self::new(
            EngineUsageAuthentication::Unknown,
            None,
            QuotaSurface::Unsupported,
            failure,
        )
    }
}

impl From<UsageReaderError> for ReaderFailure {
    fn from(error: UsageReaderError) -> Self {
        match error {
            UsageReaderError::Spawn => {
                Self::unavailable("provider usage child could not be spawned")
            }
            UsageReaderError::Timeout => Self::unavailable("provider usage read timed out"),
            UsageReaderError::Closed => {
                Self::unavailable("provider usage child closed stdio before answering")
            }
            UsageReaderError::TooLarge => {
                Self::protocol("provider usage output exceeded its byte bound")
            }
            UsageReaderError::Malformed => Self::protocol("provider usage output was malformed"),
            UsageReaderError::Protocol => {
                Self::protocol("provider usage child violated its protocol contract")
            }
            UsageReaderError::ExitStatus => {
                Self::unavailable("provider usage child exited unsuccessfully")
            }
            UsageReaderError::Empty => {
                Self::unavailable("provider usage child reported no usable data")
            }
        }
    }
}

impl From<CursorUsageError> for ReaderFailure {
    fn from(error: CursorUsageError) -> Self {
        match error {
            CursorUsageError::TokenIo => {
                Self::unavailable("cursor credential file could not be read")
            }
            CursorUsageError::TokenTooLarge => {
                Self::unavailable("cursor credential file exceeds its size bound")
            }
            CursorUsageError::TokenMalformed => {
                Self::unavailable("cursor credential file is malformed")
            }
            CursorUsageError::InsecureEndpoint => {
                Self::unavailable("cursor dashboard endpoint must be https or loopback")
            }
            CursorUsageError::Timeout => Self::unavailable("cursor dashboard deadline elapsed"),
            CursorUsageError::ConnectFailed => {
                Self::unavailable("cursor dashboard connection failed")
            }
            CursorUsageError::SendFailed => Self::unavailable("cursor dashboard request failed"),
            CursorUsageError::HttpFailure => {
                Self::unavailable("cursor dashboard returned an unsuccessful status")
            }
            CursorUsageError::BodyTooLarge => {
                Self::protocol("cursor dashboard body exceeded its bound")
            }
            CursorUsageError::BodyMalformed => {
                Self::protocol("cursor dashboard body was malformed")
            }
        }
    }
}

/// One rostered provider-account usage reader.
///
/// Reads are boxed futures so synchronous CLI readers (offloaded onto the
/// blocking pool by their implementation) and asynchronous HTTP readers
/// share one fan-out path without an async-trait crate.
pub trait AccountUsageReader: fmt::Debug + Send + Sync {
    /// Returns the stable engine id reported in snapshots.
    fn engine_id(&self) -> &'static str;

    /// Returns the engine display name reported in snapshots.
    fn display_name(&self) -> &'static str;

    /// Performs one bounded non-billable provider read.
    fn read(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderUsage, ReaderFailure>> + Send + '_>>;
}

/// Bounded Codex `account/rateLimits/read` roster reader.
#[derive(Clone, Debug)]
pub struct CodexAccountUsageReader {
    config: CodexUsageConfig,
}

impl CodexAccountUsageReader {
    /// Creates a Codex roster reader from its CLI configuration.
    #[must_use]
    pub fn new(config: CodexUsageConfig) -> Self {
        Self { config }
    }
}

impl AccountUsageReader for CodexAccountUsageReader {
    fn engine_id(&self) -> &'static str {
        "codex"
    }

    fn display_name(&self) -> &'static str {
        CODEX_DISPLAY
    }

    fn read(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderUsage, ReaderFailure>> + Send + '_>> {
        let config = self.config.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || artisan_native_engine::read_codex_usage(&config))
                .await
                .map_err(|_| ReaderFailure::unavailable(READ_FAILED))?
                .map_err(ReaderFailure::from)
        })
    }
}

/// Bounded `claude -p /usage` roster reader.
#[derive(Clone, Debug)]
pub struct ClaudeAccountUsageReader {
    config: ClaudeUsageConfig,
}

impl ClaudeAccountUsageReader {
    /// Creates a Claude roster reader from its CLI configuration.
    #[must_use]
    pub fn new(config: ClaudeUsageConfig) -> Self {
        Self { config }
    }
}

impl AccountUsageReader for ClaudeAccountUsageReader {
    fn engine_id(&self) -> &'static str {
        "claude"
    }

    fn display_name(&self) -> &'static str {
        CLAUDE_DISPLAY
    }

    fn read(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderUsage, ReaderFailure>> + Send + '_>> {
        let config = self.config.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || artisan_native_engine::read_claude_usage(&config))
                .await
                .map_err(|_| ReaderFailure::unavailable(READ_FAILED))?
                .map_err(ReaderFailure::from)
        })
    }
}

/// Bounded Cursor dashboard roster reader.
#[derive(Clone, Debug)]
pub struct CursorAccountUsageReader {
    config: CursorUsageConfig,
}

impl CursorAccountUsageReader {
    /// Creates a Cursor roster reader from its dashboard configuration.
    #[must_use]
    pub fn new(config: CursorUsageConfig) -> Self {
        Self { config }
    }
}

impl AccountUsageReader for CursorAccountUsageReader {
    fn engine_id(&self) -> &'static str {
        "cursor"
    }

    fn display_name(&self) -> &'static str {
        CURSOR_DISPLAY
    }

    fn read(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderUsage, ReaderFailure>> + Send + '_>> {
        let config = self.config.clone();
        Box::pin(async move {
            read_cursor_usage(&config)
                .await
                .map_err(ReaderFailure::from)
        })
    }
}

/// Honest unsupported-surface reader for adapters without a quota API.
#[derive(Clone, Copy, Debug)]
pub struct UnsupportedAccountUsageReader {
    engine_id: &'static str,
    display_name: &'static str,
    reason: &'static str,
}

impl UnsupportedAccountUsageReader {
    /// Creates one unsupported-surface roster entry.
    #[must_use]
    pub const fn new(
        engine_id: &'static str,
        display_name: &'static str,
        reason: &'static str,
    ) -> Self {
        Self {
            engine_id,
            display_name,
            reason,
        }
    }
}

impl AccountUsageReader for UnsupportedAccountUsageReader {
    fn engine_id(&self) -> &'static str {
        self.engine_id
    }

    fn display_name(&self) -> &'static str {
        self.display_name
    }

    fn read(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderUsage, ReaderFailure>> + Send + '_>> {
        let failure = ReaderFailure::unsupported(self.reason);
        Box::pin(async move { Err(failure) })
    }
}

/// Cloneable account-usage fan-out service with per-engine freshness.
#[derive(Debug)]
pub struct AccountUsageService {
    readers: Vec<Arc<dyn AccountUsageReader>>,
    cache: Mutex<HashMap<String, CachedUsage>>,
    freshness: Duration,
    per_engine_timeout: Duration,
}

/// One cached provider observation with its own fetch time.
///
/// `observed` drives TTL expiry; `fetched_at` is the ISO instant stamped on
/// snapshots served from this entry, so a last-good report served after a
/// failed refresh keeps its original observation time instead of the
/// current clock.
#[derive(Clone, Debug)]
struct CachedUsage {
    observed: Instant,
    fetched_at: String,
    report: EngineUsageReport,
}

impl AccountUsageService {
    /// Creates the production roster: Codex, Claude, Cursor, then the
    /// unsupported Grok Build, Hermes, and `OpenCode` entries.
    #[must_use]
    pub fn with_defaults(codex: &CliLaunch, claude: &CliLaunch, cursor: CursorUsageConfig) -> Self {
        Self::with_readers(
            vec![
                Arc::new(CodexAccountUsageReader::new(CodexUsageConfig::launched(
                    codex,
                ))) as Arc<dyn AccountUsageReader>,
                Arc::new(ClaudeAccountUsageReader::new(ClaudeUsageConfig::launched(
                    claude,
                ))) as Arc<dyn AccountUsageReader>,
                Arc::new(CursorAccountUsageReader::new(cursor)) as Arc<dyn AccountUsageReader>,
                Arc::new(UnsupportedAccountUsageReader::new(
                    "grok",
                    GROK_DISPLAY,
                    GROK_UNSUPPORTED,
                )) as Arc<dyn AccountUsageReader>,
                Arc::new(UnsupportedAccountUsageReader::new(
                    "hermes",
                    HERMES_DISPLAY,
                    HERMES_UNSUPPORTED,
                )) as Arc<dyn AccountUsageReader>,
                Arc::new(UnsupportedAccountUsageReader::new(
                    "opencode2",
                    OPENCODE2_DISPLAY,
                    OPENCODE2_UNSUPPORTED,
                )) as Arc<dyn AccountUsageReader>,
            ],
            ACCOUNT_USAGE_FRESHNESS,
            ACCOUNT_USAGE_PER_ENGINE_TIMEOUT,
        )
    }

    /// Creates a service over an explicit reader roster and bounds.
    ///
    /// Tests inject scripted readers here; production uses
    /// [`Self::with_defaults`].
    #[must_use]
    pub fn with_readers(
        readers: Vec<Arc<dyn AccountUsageReader>>,
        freshness: Duration,
        per_engine_timeout: Duration,
    ) -> Self {
        Self {
            readers,
            cache: Mutex::new(HashMap::new()),
            freshness,
            per_engine_timeout,
        }
    }

    /// Reads the requested usage snapshot with freshness and isolation.
    ///
    /// Cached reports inside the freshness window are reused unless the
    /// query forces a refresh. Every selected engine yields exactly one
    /// report; failures stay per-engine with explicit auth and surface
    /// states. A failed refresh preserves the last-good cached report with
    /// its original fetch time and marks the served copy with the refresh
    /// failure, so stale data is never stamped with the current clock. Only
    /// successful provider reads refresh the cache.
    ///
    /// # Panics
    ///
    /// Panics if a constructed snapshot violates the domain's bounded roster
    /// invariants; all inputs are pre-bounded, so this cannot occur in practice.
    #[expect(
        clippy::too_many_lines,
        reason = "one linear read over the per-engine roster; extraction would split the shared cache and failure bookkeeping"
    )]
    pub async fn read(&self, query: &ReadAccountUsage) -> EngineUsageSnapshot {
        let selected: Vec<usize> = match query.engine_id() {
            Some(engine_id) => match self
                .readers
                .iter()
                .position(|reader| reader.engine_id() == engine_id)
            {
                Some(index) => vec![index],
                None => return Self::unknown_engine_snapshot(engine_id),
            },
            None => (0..self.readers.len()).collect(),
        };
        // (report, observation time) pairs in selection order.
        let mut served: Vec<Option<(EngineUsageReport, String)>> =
            selected.iter().map(|_| None).collect();
        let mut missing: Vec<(usize, usize)> = Vec::new();
        if query.force() {
            missing = selected
                .iter()
                .enumerate()
                .map(|(slot, index)| (slot, *index))
                .collect();
        } else {
            // A panic while the cache lock was held must not take the usage
            // service down: the map holds plain report data with no
            // cross-entry invariants, so serving it after poison is safe.
            let cache = self
                .cache
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for (slot, reader_index) in selected.iter().enumerate() {
                let engine_id = self.readers[*reader_index].engine_id();
                match cache.get(engine_id) {
                    Some(cached) if cached.observed.elapsed() < self.freshness => {
                        served[slot] = Some((cached.report.clone(), cached.fetched_at.clone()));
                    }
                    _ => missing.push((slot, *reader_index)),
                }
            }
        }

        let mut outcomes: HashMap<usize, Result<ProviderUsage, ReaderFailure>> = HashMap::new();
        if !missing.is_empty() {
            let mut set: JoinSet<(usize, Result<ProviderUsage, ReaderFailure>)> = JoinSet::new();
            for (slot, reader_index) in &missing {
                let reader = Arc::clone(&self.readers[*reader_index]);
                let timeout = self.per_engine_timeout;
                let slot = *slot;
                set.spawn(async move {
                    let outcome = tokio::time::timeout(timeout, reader.read()).await;
                    let outcome = match outcome {
                        Ok(outcome) => outcome,
                        Err(_) => Err(ReaderFailure::unavailable(READ_TIMED_OUT)),
                    };
                    (slot, outcome)
                });
            }
            while let Some(joined) = set.join_next().await {
                if let Ok((slot, outcome)) = joined {
                    outcomes.insert(slot, outcome);
                } else {
                    // A panicking reader task settles as a failed read;
                    // sibling outcomes are unaffected.
                }
            }
        }

        let mut fresh: Vec<(String, CachedUsage)> = Vec::new();
        for (slot, reader_index) in selected.iter().enumerate() {
            if served[slot].is_some() {
                continue;
            }
            let reader = &self.readers[*reader_index];
            let outcome = outcomes
                .remove(&slot)
                .unwrap_or_else(|| Err(ReaderFailure::unavailable(READ_FAILED)));
            match outcome {
                Ok(usage) => {
                    let fetched_at = iso_millis(system_millis());
                    match success_report(reader, &usage) {
                        Ok(report) => {
                            fresh.push((
                                reader.engine_id().to_owned(),
                                CachedUsage {
                                    observed: Instant::now(),
                                    fetched_at: fetched_at.clone(),
                                    report: report.clone(),
                                },
                            ));
                            served[slot] = Some((report, fetched_at));
                        }
                        Err(failure) => {
                            served[slot] =
                                Some(self.last_good_or_failure(reader, &failure, fetched_at));
                        }
                    }
                }
                Err(failure) => {
                    served[slot] = Some(self.last_good_or_failure(
                        reader,
                        &failure,
                        iso_millis(system_millis()),
                    ));
                }
            }
        }
        if !fresh.is_empty() {
            let mut cache = self
                .cache
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for (engine_id, cached) in fresh {
                cache.insert(engine_id, cached);
            }
        }
        let mut reports = Vec::with_capacity(served.len());
        let mut fetched_at = String::new();
        for (report, observed_at) in served.into_iter().flatten() {
            if observed_at > fetched_at {
                fetched_at.clone_from(&observed_at);
            }
            reports.push(report);
        }
        if fetched_at.is_empty() {
            // No report was served (empty roster): stamp the empty snapshot
            // with now so construction stays total. Any served report always
            // carries its own observation time instead.
            fetched_at = iso_millis(system_millis());
        }
        // Construction invariant: `selected` is bounded by the fixed reader
        // roster and `fetched_at` comes from `iso_millis`; `read` has no typed
        // error channel to report an unrepresentable snapshot.
        EngineUsageSnapshot::new(reports, fetched_at).expect("usage roster is bounded")
    }

    /// Serves the last-good cached report with its original fetch time when
    /// one exists, marking the served copy with the refresh failure;
    /// otherwise builds a fresh failure report stamped with `now_iso`.
    fn last_good_or_failure(
        &self,
        reader: &Arc<dyn AccountUsageReader>,
        failure: &ReaderFailure,
        now_iso: String,
    ) -> (EngineUsageReport, String) {
        let cache = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(cached) = cache.get(reader.engine_id()) {
            let marked = cached
                .report
                .clone()
                .with_failure(failure.failure.clone())
                .unwrap_or_else(|_| failure_report(reader, failure));
            return (marked, cached.fetched_at.clone());
        }
        drop(cache);
        (failure_report(reader, failure), now_iso)
    }

    fn unknown_engine_snapshot(engine_id: &str) -> EngineUsageSnapshot {
        let failure = ReaderFailure::new(
            EngineUsageAuthentication::Unknown,
            None,
            QuotaSurface::Unknown,
            UNKNOWN_ENGINE,
        );
        let report = EngineUsageReport::new(
            None,
            // The failure above carries no reason; the only fallible
            // `EngineUsageAuth` input cannot occur here.
            EngineUsageAuth::new(failure.auth_state, failure.auth_reason)
                .expect("static auth state is valid"),
            engine_id.to_owned(),
            engine_id.to_owned(),
            Some(failure.failure),
            Some(failure.quota_surface),
            Vec::new(),
        )
        // The id came from a validated query and every other field is static
        // or empty; the report is within its bounds.
        .expect("unknown-engine report is bounded");
        // One report and an `iso_millis` instant are within the snapshot
        // bounds.
        EngineUsageSnapshot::new(vec![report], iso_millis(system_millis()))
            .expect("single report snapshot is bounded")
    }
}

fn success_report(
    reader: &Arc<dyn AccountUsageReader>,
    usage: &ProviderUsage,
) -> Result<EngineUsageReport, ReaderFailure> {
    EngineUsageReport::new(
        usage.account_email.clone(),
        usage.auth.clone(),
        reader.display_name().to_owned(),
        reader.engine_id().to_owned(),
        None,
        Some(usage.quota_surface),
        usage.windows.clone(),
    )
    .map_err(|_| {
        ReaderFailure::new(
            usage.auth.state(),
            usage.auth.reason().map(str::to_owned),
            usage.quota_surface,
            UNREPRESENTABLE_READ,
        )
    })
}

fn failure_report(
    reader: &Arc<dyn AccountUsageReader>,
    failure: &ReaderFailure,
) -> EngineUsageReport {
    EngineUsageReport::new(
        None,
        // The caller-owned reason is the only fallible auth input; a
        // recoverable path needs a typed error channel from this reporting
        // path (or an infallible no-reason constructor in the domain).
        EngineUsageAuth::new(failure.auth_state, failure.auth_reason.clone())
            .expect("static auth state is valid"),
        reader.display_name().to_owned(),
        reader.engine_id().to_owned(),
        Some(failure.failure.clone()),
        Some(failure.quota_surface),
        Vec::new(),
    )
    // Reader-supplied id/display text and failure text are caller-owned; a
    // malformed reader still needs a typed outcome rather than a panic.
    .expect("failure report is bounded")
}

fn system_millis() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
        })
}
