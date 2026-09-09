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
//! Engine coverage mirrors the TypeScript adapters: Codex reads
//! `account/rateLimits/read`, Claude parses `claude -p /usage`, Cursor posts
//! its dashboard endpoint, and Grok Build, Hermes, and OpenCode report
//! unsupported-with-reason because their adapters expose no account-usage
//! surface (`Engine.Usage` is optional in `modules/engines/src/engine.ts`
//! and absent from those three adapters).

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::path::PathBuf;
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
use artisan_native_engine::{ClaudeUsageConfig, CodexUsageConfig};

/// Freshness window for cached per-engine reports (60 seconds).
pub const ACCOUNT_USAGE_FRESHNESS: Duration = Duration::from_secs(60);
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
            CursorUsageError::TlsUnavailable => Self::unavailable(
                "cursor dashboard needs an https connector this build does not own",
            ),
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
    cache: Mutex<HashMap<String, (Instant, EngineUsageReport)>>,
    freshness: Duration,
    per_engine_timeout: Duration,
}

impl AccountUsageService {
    /// Creates the production roster: Codex, Claude, Cursor, then the
    /// unsupported Grok Build, Hermes, and OpenCode entries.
    #[must_use]
    pub fn with_defaults(
        codex_executable: PathBuf,
        claude_executable: PathBuf,
        cursor: CursorUsageConfig,
    ) -> Self {
        Self::with_readers(
            vec![
                Arc::new(CodexAccountUsageReader::new(CodexUsageConfig::new(
                    codex_executable,
                ))) as Arc<dyn AccountUsageReader>,
                Arc::new(ClaudeAccountUsageReader::new(ClaudeUsageConfig::new(
                    claude_executable,
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
    /// states. Only successful provider reads refresh the cache.
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
        let mut cached: Vec<Option<EngineUsageReport>> = selected.iter().map(|_| None).collect();
        let mut missing: Vec<(usize, usize)> = Vec::new();
        if !query.force() {
            let cache = self.cache.lock().expect("usage cache is not poisoned");
            for (slot, reader_index) in selected.iter().enumerate() {
                let engine_id = self.readers[*reader_index].engine_id();
                match cache.get(engine_id) {
                    Some((observed, report)) if observed.elapsed() < self.freshness => {
                        cached[slot] = Some(report.clone());
                    }
                    _ => missing.push((slot, *reader_index)),
                }
            }
        } else {
            missing = selected
                .iter()
                .enumerate()
                .map(|(slot, index)| (slot, *index))
                .collect();
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
                match joined {
                    Ok((slot, outcome)) => {
                        outcomes.insert(slot, outcome);
                    }
                    Err(_) => {
                        // A panicking reader task settles as a failed read;
                        // sibling outcomes are unaffected.
                    }
                }
            }
        }

        let mut reports: Vec<EngineUsageReport> = Vec::with_capacity(selected.len());
        let mut fresh: Vec<(String, EngineUsageReport)> = Vec::new();
        for (slot, reader_index) in selected.iter().enumerate() {
            let reader = &self.readers[*reader_index];
            if let Some(report) = cached[slot].take() {
                reports.push(report);
                continue;
            }
            let outcome = outcomes
                .remove(&slot)
                .unwrap_or_else(|| Err(ReaderFailure::unavailable(READ_FAILED)));
            let report = match outcome {
                Ok(usage) => match success_report(reader, &usage) {
                    Ok(report) => {
                        fresh.push((reader.engine_id().to_owned(), report.clone()));
                        report
                    }
                    Err(failure) => failure_report(reader, &failure),
                },
                Err(failure) => failure_report(reader, &failure),
            };
            reports.push(report);
        }
        if !fresh.is_empty() {
            let mut cache = self.cache.lock().expect("usage cache is not poisoned");
            for (engine_id, report) in fresh {
                cache.insert(engine_id, (Instant::now(), report));
            }
        }
        EngineUsageSnapshot::new(reports, iso_millis(system_millis()))
            .expect("usage roster is bounded")
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
            EngineUsageAuth::new(failure.auth_state, failure.auth_reason)
                .expect("static auth state is valid"),
            engine_id.to_owned(),
            engine_id.to_owned(),
            Some(failure.failure),
            Some(failure.quota_surface),
            Vec::new(),
        )
        .expect("unknown-engine report is bounded");
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
        EngineUsageAuth::new(failure.auth_state, failure.auth_reason.clone())
            .expect("static auth state is valid"),
        reader.display_name().to_owned(),
        reader.engine_id().to_owned(),
        Some(failure.failure.clone()),
        Some(failure.quota_surface),
        Vec::new(),
    )
    .expect("failure report is bounded")
}

fn system_millis() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}
