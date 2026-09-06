//! Durable first-message execution for configured `OpenCode2` profiles.
//!
//! This module is the one production dispatcher for the native first-turn
//! workflow. It claims a queued message, carries the immutable settings
//! snapshot through the launch fence, retains the certified profile
//! capability until the single owner spawns, binds the created provider
//! session before authorizing one prompt, and commits bounded observations
//! before issuing a wake hint. Network transcript delivery remains inside the
//! owner; this module owns only durable orchestration and its injected
//! scheduler policy.

#![forbid(unsafe_code)]

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use artisan_database::{
    AssistantChange, BindRunProvider, BindRunProviderOutcome, ClaimMessageDispatch,
    ClaimedMessageDispatch, CommitRunBatch, CommitRunBatchOutcome, CompleteRun,
    DispatchFailureReason, DispatchLeaseOwner, FailMessageDispatch, InterruptRun, LaunchClaimedRun,
    LaunchClaimedRunOutcome, LaunchedRunReceipt, ProviderBindingBytes, RecordRunUsage, Repository,
    RequeueMessageDispatch, RunBatchScope, RunErrorCode, RunErrorMessage, RunLaunchCredentials,
    RunLaunchError, RunStartKey, SessionContinuationLookup, SessionContinuationQuery,
};
use artisan_domain::{
    AssistantBody, AssistantMessagePhase, EngineId, EngineSelection, IncrementalText, ItemId,
    PatchId, Revision, RootPath, RunId, TurnId, UnixMillis,
};
use artisan_native_engine::{NativeOpenCode2Authority, VerifiedOpenCode2ProfileLaunch};
use artisan_transport::CancelHandle;
use tokio::{runtime::Handle, task::JoinHandle};

#[cfg(test)]
use crate::engine_owner::{FixtureConfiguredLaunch, FixtureTurnInput};
use crate::{
    CommandOrigin, SystemCommandOrigin,
    conversation_commit_notifier::ConversationCommitNotifier,
    engine_owner::observation::{
        EngineObservation, TerminalState, TextDelta, TextSnapshot, UsageObservation,
    },
    engine_owner::operation::{AcceptedTurn, EngineOperationError, PreparedSession, TurnResult},
    engine_owner::{EngineContinuation, EngineTurnInput},
    engine_owner::{EngineOwner, EngineOwnerShutdown},
    lifecycle_control::{ActivityGateError, ActivityGateImpl, ActivityLease},
    run_cancellation::{RunCancellationLease, RunCancellationRegistry},
    startup_reconciliation_sweep::{
        PatchSourceError, StartupReconciliationPatchSource, StartupReconciliationPatches,
        StartupReconciliationSweepInput,
    },
};

const PROMPT_DELIVERY_MAX_BYTES: usize = 256;
const PROVIDER_BINDING_VERSION: i64 = 1;
const PROVIDER_BINDING_ENGINE: &str = "opencode2";
const PROVIDER_FAILURE_CODE: &str = "provider_failed";
const PROVIDER_FAILURE_MESSAGE: &str = "OpenCode2 provider turn failed";
const INTERRUPTED_CODE: &str = "provider_interrupted";
const INTERRUPTED_MESSAGE: &str = "OpenCode2 provider turn interrupted";
const MAX_ASSISTANT_TEXT_PARTS: usize = 128;
const FIXTURE_TEXT_PART_ID: &str = "fixture-text-part";

struct AssistantTextPart {
    part_id: String,
    text: String,
}

#[derive(Default)]
struct OrderedAssistantText {
    parts: Vec<AssistantTextPart>,
    total_bytes: usize,
}

impl OrderedAssistantText {
    fn append_delta(&mut self, delta: &TextDelta) -> Option<String> {
        self.append(
            delta.part_id().unwrap_or(FIXTURE_TEXT_PART_ID),
            delta.delta(),
        )
    }

    fn replace_snapshot(&mut self, snapshot: &TextSnapshot) -> Option<String> {
        self.replace(snapshot.part_id(), snapshot.text())
    }

    fn append(&mut self, part_id: &str, text: &str) -> Option<String> {
        if part_id.is_empty() {
            return None;
        }
        let Some(index) = self.parts.iter().position(|part| part.part_id == part_id) else {
            if self.parts.len() >= MAX_ASSISTANT_TEXT_PARTS {
                return None;
            }
            let next_total = self.total_bytes.checked_add(text.len())?;
            if next_total > AssistantBody::MAX_BYTES {
                return None;
            }
            self.parts.push(AssistantTextPart {
                part_id: part_id.to_owned(),
                text: text.to_owned(),
            });
            self.total_bytes = next_total;
            return Some(self.body());
        };
        let next_total = self.total_bytes.checked_add(text.len())?;
        if next_total > AssistantBody::MAX_BYTES {
            return None;
        }
        self.parts[index].text.push_str(text);
        self.total_bytes = next_total;
        Some(self.body())
    }

    fn replace(&mut self, part_id: &str, text: &str) -> Option<String> {
        if part_id.is_empty() {
            return None;
        }
        let Some(index) = self.parts.iter().position(|part| part.part_id == part_id) else {
            if self.parts.len() >= MAX_ASSISTANT_TEXT_PARTS {
                return None;
            }
            let next_total = self.total_bytes.checked_add(text.len())?;
            if next_total > AssistantBody::MAX_BYTES {
                return None;
            }
            self.parts.push(AssistantTextPart {
                part_id: part_id.to_owned(),
                text: text.to_owned(),
            });
            self.total_bytes = next_total;
            return Some(self.body());
        };
        let previous_length = self.parts[index].text.len();
        let next_total = self
            .total_bytes
            .checked_sub(previous_length)?
            .checked_add(text.len())?;
        if next_total > AssistantBody::MAX_BYTES {
            return None;
        }
        self.parts[index].text.clear();
        self.parts[index].text.push_str(text);
        self.total_bytes = next_total;
        Some(self.body())
    }

    fn body(&self) -> String {
        let mut body = String::with_capacity(self.total_bytes);
        for part in &self.parts {
            body.push_str(&part.text);
        }
        body
    }
}

/// Decision made before any provider launch is permitted.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum SettingsLoadDecision {
    /// A validated immutable settings snapshot is ready for the launch fence.
    Ready(Box<artisan_database::ThreadEngineSettings>),
    /// The claim must be returned for a bounded later attempt.
    Requeue(&'static str),
    /// The claim contains a permanent configuration or project defect.
    Fail(&'static str),
}

/// Classifies the persisted settings read used by the production dispatcher.
///
/// This decision is intentionally separated from provider code: a missing or
/// temporarily unreadable configuration can only requeue, so no authority
/// resolution, process spawn, or session request can happen on that branch.
pub(crate) fn classify_settings_load(
    result: Result<
        Option<artisan_database::ThreadEngineSettings>,
        artisan_database::RepositoryError,
    >,
) -> SettingsLoadDecision {
    match result {
        Ok(Some(settings)) => SettingsLoadDecision::Ready(Box::new(settings)),
        Ok(None) => SettingsLoadDecision::Requeue("engine unconfigured"),
        Err(error) if is_permanent_configuration_error(&error) => {
            SettingsLoadDecision::Fail("engine settings corrupt")
        }
        Err(_) => SettingsLoadDecision::Requeue("engine settings unavailable"),
    }
}

/// Authority classification for one snapshot-fenced launch attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LaunchAuthority {
    /// This call durably created the assistant run and may contact a provider.
    Started,
    /// The durable call was replayed; no second provider effect is permitted.
    Replay,
    /// The snapshot fence rejected the attempt; the claim must be requeued.
    Requeue,
}

pub(crate) fn classify_launch_result(
    result: &Result<LaunchClaimedRunOutcome, RunLaunchError>,
) -> LaunchAuthority {
    match result {
        Ok(LaunchClaimedRunOutcome::Started(_)) => LaunchAuthority::Started,
        Ok(LaunchClaimedRunOutcome::AlreadyStarted(_)) => LaunchAuthority::Replay,
        Err(_) => LaunchAuthority::Requeue,
    }
}

/// Whether a durable provider bind authorizes the one prompt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PromptAuthorization {
    /// The current owner durably created the binding and may authorize once.
    Authorize,
    /// An unknown prior binding owns the provider session; do not prompt.
    DoNotAuthorize,
}

pub(crate) const fn prompt_authorization_after_binding(already_bound: bool) -> PromptAuthorization {
    if already_bound {
        PromptAuthorization::DoNotAuthorize
    } else {
        PromptAuthorization::Authorize
    }
}

/// Executes a notifier hint only after the caller has observed a committed
/// or idempotently replayed SQLite result.
pub(crate) fn notify_after_commit(notified_commit: bool, notify: impl FnOnce()) -> bool {
    if notified_commit {
        notify();
        true
    } else {
        false
    }
}

/// Validation failures for the explicit Forge native-run scheduler.
#[allow(clippy::module_name_repetitions)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum NativeRunDispatcherConfigError {
    /// A scheduler duration was zero.
    #[error("native run dispatcher duration must be positive")]
    ZeroDuration,
    /// A scheduler duration cannot be represented as signed milliseconds.
    #[error("native run dispatcher duration is outside the supported range")]
    DurationOverflow,
    /// The prompt delivery selector was empty or too large.
    #[error("native run dispatcher prompt delivery is outside its bound")]
    InvalidPromptDelivery,
    /// The prompt delivery selector contained a control or line-break byte.
    #[error("native run dispatcher prompt delivery contains a forbidden character")]
    InvalidPromptDeliveryCharacter,
    /// The queue capacity cannot be represented by Tokio's bounded channel.
    #[error("native run dispatcher queue capacity is outside the supported range")]
    CapacityOverflow,
}

/// Complete scheduler values supplied by the Forge composition boundary.
///
/// Every field is required in the input literal. Validation belongs to
/// [`NativeRunDispatcherConfig::new`], so no caller can accidentally create a
/// partially configured production dispatcher or introduce a hidden default.
#[allow(clippy::module_name_repetitions)]
pub struct NativeRunDispatcherConfigInput {
    /// Maximum lease lifetime for one claimed message.
    pub claim_lease: Duration,
    /// Delay between claim attempts when no work is available.
    pub poll_interval: Duration,
    /// Delay before retrying a safely requeued message.
    pub retry_backoff: Duration,
    /// Maximum time allowed for ordered dispatcher shutdown.
    pub shutdown_budget: Duration,
    /// Bounded owner admission capacity.
    pub queue_capacity: std::num::NonZeroUsize,
    /// Maximum number of retries for one identical database command.
    pub max_command_retries: std::num::NonZeroUsize,
    /// Explicit provider prompt-delivery selector.
    pub prompt_delivery: String,
    /// Provider stream cursor used for the first bounded replay.
    pub stream_after: u64,
}

impl std::fmt::Debug for NativeRunDispatcherConfigInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NativeRunDispatcherConfigInput")
            .field("claim_lease", &self.claim_lease)
            .field("poll_interval", &self.poll_interval)
            .field("retry_backoff", &self.retry_backoff)
            .field("shutdown_budget", &self.shutdown_budget)
            .field("queue_capacity", &self.queue_capacity)
            .field("max_command_retries", &self.max_command_retries)
            .field("prompt_delivery_bytes", &self.prompt_delivery.len())
            .field("stream_after", &self.stream_after)
            .finish()
    }
}

/// Explicit scheduler and provider-composition policy for one Forge process.
///
/// No field has a hidden default. The authority and notifier are both owned
/// by the dispatcher after [`Self::new`] succeeds; the caller must retain no
/// separate provider capability.
#[allow(clippy::module_name_repetitions)]
pub struct NativeRunDispatcherConfig {
    authority: NativeOpenCode2Authority,
    notifier: ConversationCommitNotifier,
    claim_lease: Duration,
    poll_interval: Duration,
    retry_backoff: Duration,
    shutdown_budget: Duration,
    queue_capacity: std::num::NonZeroUsize,
    max_command_retries: std::num::NonZeroUsize,
    prompt_delivery: String,
    stream_after: u64,
}

impl std::fmt::Debug for NativeRunDispatcherConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NativeRunDispatcherConfig")
            .field("claim_lease", &self.claim_lease)
            .field("poll_interval", &self.poll_interval)
            .field("retry_backoff", &self.retry_backoff)
            .field("shutdown_budget", &self.shutdown_budget)
            .field("queue_capacity", &self.queue_capacity)
            .field("max_command_retries", &self.max_command_retries)
            .field("prompt_delivery_bytes", &self.prompt_delivery.len())
            .field("stream_after", &self.stream_after)
            .field("authority", &"caller-selected certified authority")
            .field("notifier", &"caller-selected notifier")
            .finish()
    }
}

impl NativeRunDispatcherConfig {
    /// Clones the exact process-owned notifier for Forge request delivery.
    pub(crate) fn conversation_commit_notifier(&self) -> ConversationCommitNotifier {
        self.notifier.clone()
    }

    /// Creates a complete injected scheduler policy.
    ///
    /// # Errors
    ///
    /// Returns an error when a duration, prompt selector, or queue capacity
    /// is outside the configured scheduler bounds.
    pub fn new(
        authority: NativeOpenCode2Authority,
        notifier: ConversationCommitNotifier,
        input: NativeRunDispatcherConfigInput,
    ) -> Result<Self, NativeRunDispatcherConfigError> {
        let NativeRunDispatcherConfigInput {
            claim_lease,
            poll_interval,
            retry_backoff,
            shutdown_budget,
            queue_capacity,
            max_command_retries,
            prompt_delivery,
            stream_after,
        } = input;
        for duration in [claim_lease, poll_interval, retry_backoff, shutdown_budget] {
            if duration.is_zero() {
                return Err(NativeRunDispatcherConfigError::ZeroDuration);
            }
            if duration.as_millis() > i64::MAX as u128 {
                return Err(NativeRunDispatcherConfigError::DurationOverflow);
            }
        }
        if prompt_delivery.is_empty() || prompt_delivery.len() > PROMPT_DELIVERY_MAX_BYTES {
            return Err(NativeRunDispatcherConfigError::InvalidPromptDelivery);
        }
        if prompt_delivery
            .chars()
            .any(|character| character.is_control() || character == '\r' || character == '\n')
        {
            return Err(NativeRunDispatcherConfigError::InvalidPromptDeliveryCharacter);
        }
        if queue_capacity.get() > tokio::sync::Semaphore::MAX_PERMITS {
            return Err(NativeRunDispatcherConfigError::CapacityOverflow);
        }
        Ok(Self {
            authority,
            notifier,
            claim_lease,
            poll_interval,
            retry_backoff,
            shutdown_budget,
            queue_capacity,
            max_command_retries,
            prompt_delivery,
            stream_after,
        })
    }
}

/// The observed result of stopping the native dispatcher.
#[allow(clippy::module_name_repetitions)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeRunDispatcherShutdown {
    /// The dispatcher and its single engine owner joined cleanly.
    Joined,
    /// The configured shutdown budget elapsed, but the join was still
    /// awaited to resolve owner custody.
    BudgetExceeded,
    /// The dispatcher task or owner task was lost.
    TaskLost,
}

/// Test-only bundled parameters for the one-shot fixture pipeline.
///
/// Groups the nine fixture-launch inputs so the scenario helper stays
/// within the argument budget without a lint suppression. No production
/// path uses this type.
#[cfg(test)]
pub(crate) struct FixtureScenarioLaunch<'a> {
    pub(crate) repository: Repository,
    pub(crate) database_path: PathBuf,
    pub(crate) config: NativeRunDispatcherConfig,
    pub(crate) process_cancel: Arc<CancelHandle>,
    pub(crate) cancellation: RunCancellationRegistry,
    pub(crate) activity: ActivityGateImpl,
    pub(crate) runtime: &'a Handle,
    pub(crate) fixture_program: PathBuf,
    pub(crate) scenario: &'static str,
}

/// One running configured first-message dispatcher.
#[allow(clippy::module_name_repetitions)]
pub struct NativeRunDispatcher {
    catalog_client: crate::engine_owner::EngineCatalogClient,
    stop: Arc<CancelHandle>,
    shutdown_budget: Duration,
    join: Option<JoinHandle<DispatchLoopExit>>,
    observed: Option<NativeRunDispatcherShutdown>,
}

/// The production dispatcher resolves a certified profile for every claim.
/// The fixture variant exists only in test builds and is consumed before the
/// one claim it is allowed to authorize.
enum DispatchLaunchMode {
    Configured,
    #[cfg(test)]
    Fixture(Option<FixtureConfiguredLaunch>),
}

enum ClaimLaunchMode {
    Configured,
    #[cfg(test)]
    Fixture(FixtureConfiguredLaunch),
}

enum ClaimLaunchAvailability {
    Available(ClaimLaunchMode),
    Exhausted,
}

enum ResolvedLaunch {
    Configured(Box<VerifiedOpenCode2ProfileLaunch>),
    #[cfg(test)]
    Fixture(FixtureConfiguredLaunch),
}

impl DispatchLaunchMode {
    fn claim_for_next(&mut self) -> ClaimLaunchAvailability {
        match self {
            Self::Configured => ClaimLaunchAvailability::Available(ClaimLaunchMode::Configured),
            #[cfg(test)]
            Self::Fixture(launch) => match launch.take() {
                Some(launch) => {
                    ClaimLaunchAvailability::Available(ClaimLaunchMode::Fixture(launch))
                }
                None => ClaimLaunchAvailability::Exhausted,
            },
        }
    }
}

impl std::fmt::Debug for NativeRunDispatcher {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("NativeRunDispatcher { <payload-free> }")
    }
}

impl NativeRunDispatcher {
    /// Starts the sole background dispatcher on the caller's runtime.
    #[must_use]
    pub(crate) fn start(
        repository: Repository,
        database_path: PathBuf,
        config: NativeRunDispatcherConfig,
        process_cancel: Arc<CancelHandle>,
        activity: ActivityGateImpl,
        runtime: &Handle,
    ) -> Self {
        let cancellation = RunCancellationRegistry::new(config.queue_capacity.get())
            .expect("dispatcher queue capacity should be nonzero");
        Self::start_with_registry(
            repository,
            database_path,
            config,
            process_cancel,
            cancellation,
            activity,
            runtime,
        )
    }

    /// Starts the production dispatcher with the exact registry shared by
    /// authenticated StopRun requests. The registry is created by Forge
    /// assembly and never replaced for the dispatcher's lifetime.
    #[must_use]
    pub(crate) fn start_with_registry(
        repository: Repository,
        database_path: PathBuf,
        config: NativeRunDispatcherConfig,
        process_cancel: Arc<CancelHandle>,
        cancellation: RunCancellationRegistry,
        activity: ActivityGateImpl,
        runtime: &Handle,
    ) -> Self {
        Self::start_with_mode(
            repository,
            database_path,
            config,
            process_cancel,
            cancellation,
            activity,
            runtime,
            DispatchLaunchMode::Configured,
        )
    }

    /// Starts the common dispatcher pipeline against the registered protocol
    /// fixture. This launch mode is test-only and one-shot by construction.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn start_with_fixture_for_tests(
        repository: Repository,
        database_path: PathBuf,
        config: NativeRunDispatcherConfig,
        process_cancel: Arc<CancelHandle>,
        activity: ActivityGateImpl,
        runtime: &Handle,
        fixture_program: PathBuf,
    ) -> Self {
        Self::start_with_fixture_scenario_for_tests(FixtureScenarioLaunch {
            repository,
            database_path,
            config,
            process_cancel,
            cancellation: RunCancellationRegistry::new(1)
                .expect("fixture cancellation capacity should be nonzero"),
            activity,
            runtime,
            fixture_program,
            scenario: "prompt_text_then_terminal",
        })
    }

    /// Starts the same one-shot fixture pipeline with an explicit frozen
    /// fixture scenario (for example the deterministic hold-after-first-delta
    /// variant). Test-only; production always uses [`Self::start`].
    #[cfg(test)]
    #[must_use]
    pub(crate) fn start_with_fixture_scenario_for_tests(params: FixtureScenarioLaunch<'_>) -> Self {
        let FixtureScenarioLaunch {
            repository,
            database_path,
            config,
            process_cancel,
            cancellation,
            activity,
            runtime,
            fixture_program,
            scenario,
        } = params;
        Self::start_with_mode(
            repository,
            database_path,
            config,
            process_cancel,
            cancellation,
            activity,
            runtime,
            DispatchLaunchMode::Fixture(Some(FixtureConfiguredLaunch {
                program: fixture_program,
                version: "0.0.0-fixture",
                profile_id: "fixture-test".to_owned(),
                scenario,
            })),
        )
    }

    fn start_with_mode(
        repository: Repository,
        database_path: PathBuf,
        config: NativeRunDispatcherConfig,
        process_cancel: Arc<CancelHandle>,
        cancellation: RunCancellationRegistry,
        activity: ActivityGateImpl,
        runtime: &Handle,
        launch_mode: DispatchLaunchMode,
    ) -> Self {
        let shutdown_budget = config.shutdown_budget;
        let stop = Arc::new(CancelHandle::new());
        let owner = EngineOwner::start_configured(config.queue_capacity, runtime);
        let catalog_client = owner.catalog_client();
        let join = runtime.spawn(dispatch_loop(DispatchLoopContext {
            repository,
            database_path,
            config,
            stop: Arc::clone(&stop),
            process_cancel,
            cancellation,
            owner,
            activity,
            launch_mode,
        }));
        Self {
            stop,
            shutdown_budget,
            catalog_client,
            join: Some(join),
            observed: None,
        }
    }

    pub(crate) fn catalog_client(&self) -> crate::engine_owner::EngineCatalogClient {
        self.catalog_client.clone()
    }

    /// Stops claims, drains admission, and awaits the owner. A budget breach
    /// is reported after the join is nevertheless awaited so child custody is
    /// never detached from this shutdown path.
    pub(crate) async fn shutdown(&mut self) -> NativeRunDispatcherShutdown {
        self.stop.cancel();
        if let Some(observed) = self.observed {
            return observed;
        }
        let Some(join) = self.join.take() else {
            self.observed = Some(NativeRunDispatcherShutdown::Joined);
            return NativeRunDispatcherShutdown::Joined;
        };
        let mut join = join;
        let result = tokio::time::timeout(self.shutdown_budget, &mut join).await;
        let (budget_exceeded, join_result) = match result {
            Ok(join_result) => (false, join_result),
            Err(_) => (true, join.await),
        };
        let outcome = match join_result {
            Ok(DispatchLoopExit {
                owner: EngineOwnerShutdown::Joined,
            }) if !budget_exceeded => NativeRunDispatcherShutdown::Joined,
            Ok(DispatchLoopExit {
                owner: EngineOwnerShutdown::Joined,
            }) => NativeRunDispatcherShutdown::BudgetExceeded,
            Ok(DispatchLoopExit {
                owner: EngineOwnerShutdown::Quarantined | EngineOwnerShutdown::TaskLost,
            })
            | Err(_) => NativeRunDispatcherShutdown::TaskLost,
        };
        self.observed = Some(outcome);
        outcome
    }
}

impl Drop for NativeRunDispatcher {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

struct DispatchLoopExit {
    owner: EngineOwnerShutdown,
}

struct DispatchLoopContext {
    repository: Repository,
    database_path: PathBuf,
    config: NativeRunDispatcherConfig,
    stop: Arc<CancelHandle>,
    process_cancel: Arc<CancelHandle>,
    cancellation: RunCancellationRegistry,
    owner: EngineOwner,
    activity: ActivityGateImpl,
    launch_mode: DispatchLaunchMode,
}

struct LiveRecoveryPatchSource {
    notifier: ConversationCommitNotifier,
}

impl StartupReconciliationPatchSource for LiveRecoveryPatchSource {
    fn patch_ids_for(
        &mut self,
        candidate: &artisan_database::StartupReconciliationCandidate,
    ) -> Result<StartupReconciliationPatches, PatchSourceError> {
        let turn_patch_id =
            PatchId::parse(candidate.run_id.as_str()).map_err(|_| PatchSourceError)?;
        let item_patch_id = candidate
            .assistant_item_id
            .as_ref()
            .map(|item_id| PatchId::parse(item_id.as_str()).map_err(|_| PatchSourceError))
            .transpose()?;
        Ok(StartupReconciliationPatches::new(
            turn_patch_id,
            item_patch_id,
        ))
    }

    fn on_durable_disposition(
        &mut self,
        candidate: &artisan_database::StartupReconciliationCandidate,
    ) {
        let _ = self.notifier.publish(&candidate.thread_id);
    }
}

async fn perform_live_recovery_page(
    repository: &Repository,
    config: &NativeRunDispatcherConfig,
    operated_at: UnixMillis,
) -> Result<
    crate::startup_reconciliation_sweep::StartupReconciliationSweepReport,
    Box<crate::startup_reconciliation_sweep::StartupReconciliationSweepError>,
> {
    let input = StartupReconciliationSweepInput::new(operated_at, 64).map_err(Box::new)?;
    let mut source = LiveRecoveryPatchSource {
        notifier: config.conversation_commit_notifier(),
    };
    crate::startup_reconciliation_sweep::sweep_startup_reconciliation(
        repository,
        input,
        &mut source,
    )
    .await
    .map_err(Box::new)
}

async fn run_recovery_pages(
    repository: &Repository,
    config: &NativeRunDispatcherConfig,
    origin: &SystemCommandOrigin,
    stop: &CancelHandle,
    process_cancel: &CancelHandle,
) -> bool {
    loop {
        if stop.is_cancelled() || process_cancel.is_cancelled() {
            return false;
        }
        let Some(operated_at) = wall_clock(origin) else {
            if !wait_for_next_claim(stop, process_cancel, config.poll_interval).await {
                return false;
            }
            return false;
        };
        if let Ok(report) = perform_live_recovery_page(repository, config, operated_at).await {
            if report.discovered == 64 {
                if !wait_for_next_claim(stop, process_cancel, config.poll_interval).await {
                    return false;
                }
                continue;
            }
            return true;
        }
        if !wait_for_next_claim(stop, process_cancel, config.poll_interval).await {
            return false;
        }
        return false;
    }
}

async fn run_final_recovery_page(
    repository: &Repository,
    config: &NativeRunDispatcherConfig,
    origin: &SystemCommandOrigin,
) {
    let Some(operated_at) = wall_clock(origin) else {
        return;
    };
    let _ = perform_live_recovery_page(repository, config, operated_at).await;
}

async fn shutdown_owner_until_settled(owner: &mut EngineOwner) -> EngineOwnerShutdown {
    loop {
        let outcome = owner.shutdown().await;
        if !matches!(outcome, EngineOwnerShutdown::Quarantined) {
            return outcome;
        }
    }
}

async fn dispatch_loop(context: DispatchLoopContext) -> DispatchLoopExit {
    let DispatchLoopContext {
        repository,
        database_path,
        config,
        stop,
        process_cancel,
        cancellation,
        mut owner,
        activity,
        mut launch_mode,
    } = context;
    let origin = SystemCommandOrigin;
    // `AcceptedTurn::finish` can report an unresolved reap while the owner
    // quarantines a retained child. Keep its activity lease until owner
    // shutdown proves that custody has resolved.
    let mut retained_activity = Vec::new();
    loop {
        if stop.is_cancelled() || process_cancel.is_cancelled() {
            break;
        }
        let proceed =
            run_recovery_pages(&repository, &config, &origin, &stop, &process_cancel).await;
        if stop.is_cancelled() || process_cancel.is_cancelled() {
            break;
        }
        if !proceed {
            continue;
        }
        let activity_lease = match activity.acquire() {
            Ok(lease) => lease,
            Err(ActivityGateError::Unavailable | ActivityGateError::CountOutOfRange) => {
                if !wait_for_next_claim(&stop, &process_cancel, config.poll_interval).await {
                    break;
                }
                continue;
            }
        };
        // Consuming the fixture capability after activity admission makes its
        // fixed run identity one-shot without opening a stop race.
        let launch_mode = match launch_mode.claim_for_next() {
            ClaimLaunchAvailability::Available(launch_mode) => launch_mode,
            ClaimLaunchAvailability::Exhausted => {
                drop(activity_lease);
                break;
            }
        };
        let Some(claimed_at) = wall_clock(&origin) else {
            drop(activity_lease);
            if !wait_for_next_claim(&stop, &process_cancel, config.poll_interval).await {
                break;
            }
            continue;
        };
        let Some(lease_expires_at) = add_duration(claimed_at, config.claim_lease) else {
            drop(activity_lease);
            if !wait_for_next_claim(&stop, &process_cancel, config.poll_interval).await {
                break;
            }
            continue;
        };
        let Some(owner_token) = mint_dispatch_owner() else {
            drop(activity_lease);
            if !wait_for_next_claim(&stop, &process_cancel, config.poll_interval).await {
                break;
            }
            continue;
        };
        let claim = ClaimMessageDispatch {
            owner: owner_token,
            claimed_at,
            lease_expires_at,
        };
        let Ok(Some(claimed)) = repository.claim_next_message_dispatch(claim).await else {
            drop(activity_lease);
            if !wait_for_next_claim(&stop, &process_cancel, config.poll_interval).await {
                break;
            }
            continue;
        };
        if let Some(lease) = execute_claim(
            ClaimExecution {
                repository: &repository,
                database_path: Path::new(&database_path),
                config: &config,
                origin: &origin,
                stop: &stop,
                process_cancel: &process_cancel,
                cancellation: &cancellation,
                owner: &owner,
                claimed,
            },
            launch_mode,
            activity_lease,
        )
        .await
        {
            retained_activity.push(lease);
        }
    }

    run_final_recovery_page(&repository, &config, &origin).await;

    let owner_shutdown = shutdown_owner_until_settled(&mut owner).await;
    // The owner shutdown loop above does not complete while unresolved child
    // custody remains, so releasing these leases is safe at this boundary.
    drop(retained_activity);
    DispatchLoopExit {
        owner: owner_shutdown,
    }
}

async fn wait_for_next_claim(
    stop: &CancelHandle,
    process_cancel: &CancelHandle,
    interval: Duration,
) -> bool {
    tokio::select! {
        biased;
        () = stop.wait() => false,
        () = process_cancel.wait() => false,
        () = tokio::time::sleep(interval) => true,
    }
}

fn wall_clock(origin: &SystemCommandOrigin) -> Option<UnixMillis> {
    origin.acceptance_instant().ok()
}

fn add_duration(value: UnixMillis, duration: Duration) -> Option<UnixMillis> {
    let milliseconds = i64::try_from(duration.as_millis()).ok()?;
    value
        .as_millis()
        .checked_add(milliseconds)
        .map(UnixMillis::from_millis)
}

fn at_or_after(origin: &SystemCommandOrigin, not_before: UnixMillis) -> Option<UnixMillis> {
    Some(wall_clock(origin)?.max(not_before))
}

fn mint_dispatch_owner() -> Option<DispatchLeaseOwner> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).ok()?;
    Some(DispatchLeaseOwner::new(bytes))
}

fn mint_run_capabilities() -> Option<(RunStartKey, RunLaunchCredentials)> {
    let mut start = [0_u8; 32];
    let mut owner = [0_u8; 32];
    let mut lease = [0_u8; 32];
    let mut claim = [0_u8; 32];
    getrandom::fill(&mut start).ok()?;
    getrandom::fill(&mut owner).ok()?;
    getrandom::fill(&mut lease).ok()?;
    getrandom::fill(&mut claim).ok()?;
    Some((
        RunStartKey::new(start),
        RunLaunchCredentials::new(owner, lease, claim),
    ))
}

fn mint_run_id(origin: &SystemCommandOrigin) -> Option<RunId> {
    RunId::parse(origin.mint_identity().ok()?).ok()
}

fn mint_turn_id(origin: &SystemCommandOrigin) -> Option<TurnId> {
    TurnId::parse(origin.mint_identity().ok()?).ok()
}

fn mint_item_id(origin: &SystemCommandOrigin) -> Option<ItemId> {
    ItemId::parse(origin.mint_identity().ok()?).ok()
}

fn mint_patch_id(origin: &SystemCommandOrigin) -> Option<PatchId> {
    PatchId::parse(origin.mint_identity().ok()?).ok()
}

struct ClaimExecution<'a> {
    repository: &'a Repository,
    database_path: &'a Path,
    config: &'a NativeRunDispatcherConfig,
    origin: &'a SystemCommandOrigin,
    stop: &'a CancelHandle,
    process_cancel: &'a CancelHandle,
    cancellation: &'a RunCancellationRegistry,
    owner: &'a EngineOwner,
    claimed: ClaimedMessageDispatch,
}

impl ClaimExecution<'_> {
    async fn requeue(self, reason: &'static str) {
        requeue_claim(
            self.repository,
            self.claimed,
            self.config,
            self.origin,
            reason,
        )
        .await;
    }

    async fn fail(self, reason: &'static str) {
        fail_claim(self.repository, self.claimed, self.origin, reason).await;
    }
}

struct LoadedClaim<'a> {
    context: ClaimExecution<'a>,
    payload: artisan_database::QueueMessageDispatchPayload,
    settings: artisan_database::ThreadEngineSettings,
    project_root: RootPath,
    launch: ResolvedLaunch,
}

struct ClaimIds {
    run_id: RunId,
    turn_id: TurnId,
    item_id: ItemId,
    first_patch_id: PatchId,
    second_patch_id: PatchId,
    operated_at: UnixMillis,
    run_start_key: RunStartKey,
    credentials: RunLaunchCredentials,
}

struct LaunchedClaim<'a> {
    context: ClaimExecution<'a>,
    payload: artisan_database::QueueMessageDispatchPayload,
    settings: artisan_database::ThreadEngineSettings,
    project_root: RootPath,
    launch: ResolvedLaunch,
    continuation: Option<EngineContinuation>,
    ids: ClaimIds,
    receipt: LaunchedRunReceipt,
    cancellation: RunCancellationLease,
}

struct PreparedClaim<'a> {
    context: ClaimExecution<'a>,
    ids: ClaimIds,
    receipt: LaunchedRunReceipt,
    settings: artisan_database::ThreadEngineSettings,
    turn: AcceptedTurn,
    session: PreparedSession,
    cancellation: RunCancellationLease,
}

struct BoundClaim<'a> {
    context: ClaimExecution<'a>,
    ids: ClaimIds,
    receipt: LaunchedRunReceipt,
    bound: artisan_database::BoundRunReceipt,
    bound_at: UnixMillis,
    turn: AcceptedTurn,
    cancellation: RunCancellationLease,
}

enum ClaimCustody {
    Released,
    Retained(RunCancellationLease),
}

struct RetainedActivity {
    _activity: ActivityLease,
    _cancellation: RunCancellationLease,
}

async fn execute_claim(
    context: ClaimExecution<'_>,
    launch_mode: ClaimLaunchMode,
    activity_lease: ActivityLease,
) -> Option<RetainedActivity> {
    if context.stop.is_cancelled() || context.process_cancel.is_cancelled() {
        context.requeue("dispatcher stopping").await;
        return None;
    }
    let loaded = load_claim(context, launch_mode).await?;
    let ids = match mint_claim_ids(
        loaded.context.origin,
        loaded.context.claimed.updated_at,
        &loaded.launch,
    ) {
        Ok(ids) => ids,
        Err(reason) => {
            loaded.context.requeue(reason).await;
            return None;
        }
    };
    let continuation = match resolve_continuation(&loaded, &ids).await {
        Ok(continuation) => continuation,
        Err(reason) => {
            loaded.context.fail(reason).await;
            return None;
        }
    };
    let cancellation = match loaded
        .context
        .cancellation
        .register(loaded.payload.thread_id.clone(), ids.run_id.clone())
    {
        Ok(lease) => lease,
        Err(_) => {
            loaded.context.requeue("run cancellation unavailable").await;
            return None;
        }
    };
    let launched = launch_claim(loaded, ids, cancellation, continuation).await?;
    let (prepared, custody) = admit_claim(launched).await;
    let Some(prepared) = prepared else {
        return match custody {
            ClaimCustody::Released => None,
            ClaimCustody::Retained(cancellation) => Some(RetainedActivity {
                _activity: activity_lease,
                _cancellation: cancellation,
            }),
        };
    };
    let (bound, custody) = bind_claim(prepared).await;
    let Some(bound) = bound else {
        return match custody {
            ClaimCustody::Released => None,
            ClaimCustody::Retained(cancellation) => Some(RetainedActivity {
                _activity: activity_lease,
                _cancellation: cancellation,
            }),
        };
    };
    match consume_bound_claim(bound).await {
        ClaimCustody::Released => None,
        ClaimCustody::Retained(cancellation) => Some(RetainedActivity {
            _activity: activity_lease,
            _cancellation: cancellation,
        }),
    }
}

async fn load_claim(
    context: ClaimExecution<'_>,
    launch_mode: ClaimLaunchMode,
) -> Option<LoadedClaim<'_>> {
    let Some(payload) = read_payload(context.repository, &context.claimed).await else {
        context.requeue("message payload unavailable").await;
        return None;
    };
    let settings = match classify_settings_load(
        context
            .repository
            .read_thread_engine_settings(&payload.thread_id)
            .await,
    ) {
        SettingsLoadDecision::Ready(settings) => *settings,
        SettingsLoadDecision::Requeue(reason) => {
            context.requeue(reason).await;
            return None;
        }
        SettingsLoadDecision::Fail(reason) => {
            context.fail(reason).await;
            return None;
        }
    };
    let project_root = match context
        .repository
        .read_thread_project_root(&payload.thread_id)
        .await
    {
        Ok(root) => root,
        Err(error) => {
            if is_permanent_configuration_error(&error) {
                context.fail("project root corrupt").await;
            } else {
                context.requeue("project root unavailable").await;
            }
            return None;
        }
    };
    // Only an OpenCode2 selection can reach the certified profile launch
    // below. Newly representable engines stay explicitly unavailable: the
    // claim requeues instead of running as OpenCode2.
    let EngineSelection::OpenCode2(selection) = settings.config().selection() else {
        context.requeue("engine unavailable").await;
        return None;
    };
    let launch = match launch_mode {
        ClaimLaunchMode::Configured => {
            let profile_id = selection.profile_id();
            let Ok(launch) = context
                .config
                .authority
                .resolve_profile_launch(context.database_path, profile_id)
            else {
                context.requeue("engine profile unavailable").await;
                return None;
            };
            ResolvedLaunch::Configured(Box::new(launch))
        }
        #[cfg(test)]
        ClaimLaunchMode::Fixture(fixture) => {
            let configured_profile = selection.profile_id();
            if configured_profile.as_str() != fixture.profile_id.as_str() {
                context.requeue("engine profile unavailable").await;
                return None;
            }
            ResolvedLaunch::Fixture(fixture)
        }
    };
    Some(LoadedClaim {
        context,
        payload,
        settings,
        project_root,
        launch,
    })
}

async fn resolve_continuation(
    claim: &LoadedClaim<'_>,
    ids: &ClaimIds,
) -> Result<Option<EngineContinuation>, &'static str> {
    #[cfg(test)]
    if matches!(&claim.launch, ResolvedLaunch::Fixture(_)) {
        return Ok(None);
    }
    let EngineSelection::OpenCode2(selection) = claim.settings.config().selection() else {
        return Err("engine unavailable");
    };
    let profile_id = selection.profile_id().clone();
    let lookup = claim
        .context
        .repository
        .read_session_continuation(SessionContinuationQuery {
            thread_id: claim.payload.thread_id.clone(),
            engine_id: EngineId::OpenCode2,
            profile_id,
            exclude_run_id: Some(ids.run_id.clone()),
        })
        .await
        .map_err(|_| "provider continuation lookup failed")?;
    match lookup {
        SessionContinuationLookup::NoHistory => Ok(None),
        SessionContinuationLookup::Usable(continuation) => {
            EngineContinuation::new(continuation.session_id.as_str().to_owned())
                .map(Some)
                .ok_or("provider continuation corrupt")
        }
        SessionContinuationLookup::Unavailable(_) => Err("provider continuation unavailable"),
        SessionContinuationLookup::Incompatible(_) => Err("provider continuation incompatible"),
    }
}

fn mint_claim_ids(
    origin: &SystemCommandOrigin,
    updated_at: UnixMillis,
    launch: &ResolvedLaunch,
) -> Result<ClaimIds, &'static str> {
    let (run_id, turn_id, item_id, first_patch_id, second_patch_id) = match launch {
        ResolvedLaunch::Configured(_) => (
            mint_run_id(origin).ok_or("run identity unavailable")?,
            mint_turn_id(origin).ok_or("run identity unavailable")?,
            mint_item_id(origin).ok_or("run identity unavailable")?,
            mint_patch_id(origin).ok_or("run identity unavailable")?,
            mint_patch_id(origin).ok_or("run identity unavailable")?,
        ),
        #[cfg(test)]
        ResolvedLaunch::Fixture(_) => fixture_claim_ids()?,
    };
    let operated_at = at_or_after(origin, updated_at).ok_or("run clock unavailable")?;
    let (run_start_key, credentials) =
        mint_run_capabilities().ok_or("run capability unavailable")?;
    Ok(ClaimIds {
        run_id,
        turn_id,
        item_id,
        first_patch_id,
        second_patch_id,
        operated_at,
        run_start_key,
        credentials,
    })
}

#[cfg(test)]
fn fixture_claim_ids() -> Result<(RunId, TurnId, ItemId, PatchId, PatchId), &'static str> {
    Ok((
        RunId::parse("fixture-run").map_err(|_| "run identity unavailable")?,
        TurnId::parse("fixture-turn").map_err(|_| "run identity unavailable")?,
        ItemId::parse("fixture-user-item").map_err(|_| "run identity unavailable")?,
        PatchId::parse("fixture-launch-turn").map_err(|_| "run identity unavailable")?,
        PatchId::parse("fixture-launch-item").map_err(|_| "run identity unavailable")?,
    ))
}

async fn launch_claim(
    loaded: LoadedClaim<'_>,
    ids: ClaimIds,
    cancellation: RunCancellationLease,
    continuation: Option<EngineContinuation>,
) -> Option<LaunchedClaim<'_>> {
    let launch_result = launch_with_retry(
        loaded.context.repository,
        LaunchClaimedRun {
            claimed: &loaded.context.claimed,
            run_id: &ids.run_id,
            turn_id: &ids.turn_id,
            item_id: &ids.item_id,
            first_patch_id: &ids.first_patch_id,
            second_patch_id: &ids.second_patch_id,
            operated_at: ids.operated_at,
            run_start_key: &ids.run_start_key,
            credentials: &ids.credentials,
            engine_settings: &loaded.settings,
        },
        loaded.context.config.max_command_retries,
    )
    .await;
    let receipt = match classify_launch_result(&launch_result) {
        LaunchAuthority::Started => match launch_result {
            Ok(LaunchClaimedRunOutcome::Started(receipt)) => receipt,
            _ => unreachable!("started launch authority has a started receipt"),
        },
        // `AlreadyStarted` is durable replay information, never authority to
        // contact OpenCode. Leave the launching run for the recovery path;
        // creating another provider session here could duplicate an unknown
        // external effect from the original attempt.
        LaunchAuthority::Replay => return None,
        LaunchAuthority::Requeue => {
            loaded.context.requeue("run launch unavailable").await;
            return None;
        }
    };
    let LoadedClaim {
        context,
        payload,
        settings,
        project_root,
        launch,
    } = loaded;
    Some(LaunchedClaim {
        context,
        payload,
        settings,
        project_root,
        launch,
        continuation,
        ids,
        receipt,
        cancellation,
    })
}

async fn admit_claim(claim: LaunchedClaim<'_>) -> (Option<PreparedClaim<'_>>, ClaimCustody) {
    let LaunchedClaim {
        context,
        payload,
        settings,
        project_root,
        launch,
        continuation,
        ids,
        receipt,
        cancellation,
    } = claim;
    let attempt_budget = Duration::from_millis(settings.config().runtime().attempt_budget().get());
    let prompt_id = payload.message_id.as_str().to_owned();
    let prompt = payload.payload;
    let prompt_delivery = context.config.prompt_delivery.clone();
    let stream_after = context.config.stream_after;
    let control_capacity = context.config.queue_capacity.get();
    let run_cancel = cancellation.cancel_handle();
    let turn_result = match launch {
        ResolvedLaunch::Configured(launch) => context.owner.admit_turn(
            EngineTurnInput {
                run_id: receipt.run_id.clone(),
                thread_id: payload.thread_id.clone(),
                project_root,
                prompt_id,
                prompt,
                settings: settings.clone(),
                launch: *launch,
                continuation,
                prompt_delivery,
                stream_after,
                control_capacity,
            },
            attempt_budget,
        ),
        #[cfg(test)]
        ResolvedLaunch::Fixture(fixture) => context.owner.admit_fixture_turn(
            FixtureTurnInput {
                run_id: receipt.run_id.clone(),
                project_root,
                prompt_id,
                prompt,
                settings: settings.clone(),
                fixture,
                prompt_delivery,
                stream_after,
                control_capacity,
            },
            attempt_budget,
        ),
    };
    let Ok(mut turn) = turn_result else {
        return (None, ClaimCustody::Released);
    };
    let preparation = tokio::select! {
        biased;
        result = turn.prepare() => Ok(result),
        () = run_cancel.wait() => Err(()),
    };
    let (session_result, cancellation_observed) = match preparation {
        Ok(result) => (result, run_cancel.is_cancelled()),
        Err(()) => {
            // The lease signal wins without dropping the AcceptedTurn. Keep
            // setup alive long enough to obtain the session needed for the
            // durable bind, then cancel before authorization. This preserves
            // a user-cancelled terminal path instead of leaving an unbound
            // launching row for interruption recovery.
            (turn.prepare().await, true)
        }
    };
    let Ok(session) = session_result else {
        let custody = if is_unresolved_reap(&turn.finish().await) {
            ClaimCustody::Retained(cancellation)
        } else {
            ClaimCustody::Released
        };
        return (None, custody);
    };
    if cancellation_observed {
        turn.cancel();
    }
    (
        Some(PreparedClaim {
            context,
            ids,
            receipt,
            settings,
            turn,
            session,
            cancellation,
        }),
        ClaimCustody::Released,
    )
}

async fn bind_claim(claim: PreparedClaim<'_>) -> (Option<BoundClaim<'_>>, ClaimCustody) {
    let PreparedClaim {
        context,
        ids,
        receipt,
        settings,
        mut turn,
        session,
        cancellation,
    } = claim;
    let run_cancel = cancellation.cancel_handle();
    if run_cancel.is_cancelled() {
        turn.cancel();
    }
    // Provider binding bytes are OpenCode2-shaped. A selection for any other
    // engine abandons the turn here instead of binding as OpenCode2.
    let EngineSelection::OpenCode2(selection) = settings.config().selection() else {
        let custody = abandon_turn(turn, context.stop, context.process_cancel).await;
        return (
            None,
            if custody {
                ClaimCustody::Retained(cancellation)
            } else {
                ClaimCustody::Released
            },
        );
    };
    let Some(binding_bytes) =
        provider_binding_bytes(selection.profile_id().as_str(), session.session())
    else {
        let custody = abandon_turn(turn, context.stop, context.process_cancel).await;
        return (
            None,
            if custody {
                ClaimCustody::Retained(cancellation)
            } else {
                ClaimCustody::Released
            },
        );
    };
    let Some(bound_at) = at_or_after(context.origin, ids.operated_at) else {
        let custody = abandon_turn(turn, context.stop, context.process_cancel).await;
        return (
            None,
            if custody {
                ClaimCustody::Retained(cancellation)
            } else {
                ClaimCustody::Released
            },
        );
    };
    let bind_command = || BindRunProvider {
        claimed: &context.claimed,
        receipt: &receipt,
        run_start_key: &ids.run_start_key,
        credentials: &ids.credentials,
        expected_launch_at: ids.operated_at,
        bound_at,
        binding_version: PROVIDER_BINDING_VERSION,
        binding_bytes: &binding_bytes,
    };
    let bind_result = tokio::select! {
        biased;
        result = bind_with_retry(
            context.repository,
            bind_command(),
            context.config.max_command_retries,
        ) => result,
        () = run_cancel.wait() => {
            // Do not drop the AcceptedTurn while the durable bind is pending.
            // Cancel the provider operation, then finish the same idempotent
            // bind path so terminal settlement has an authenticated scope.
            turn.cancel();
            bind_with_retry(
                context.repository,
                bind_command(),
                context.config.max_command_retries,
            )
            .await
        }
    };
    let (bound, already_bound) = match bind_result {
        Ok(BindRunProviderOutcome::Bound(receipt)) => (receipt, false),
        Ok(BindRunProviderOutcome::AlreadyBound(receipt)) => (receipt, true),
        Err(_) => {
            let custody = abandon_turn(turn, context.stop, context.process_cancel).await;
            return (
                None,
                if custody {
                    ClaimCustody::Retained(cancellation)
                } else {
                    ClaimCustody::Released
                },
            );
        }
    };
    if run_cancel.is_cancelled() {
        turn.cancel();
    }
    let authorization_failed = match prompt_authorization_after_binding(already_bound) {
        PromptAuthorization::DoNotAuthorize => true,
        PromptAuthorization::Authorize => turn.authorize().is_err(),
    };
    if authorization_failed && !run_cancel.is_cancelled() {
        let custody = abandon_turn(turn, context.stop, context.process_cancel).await;
        return (
            None,
            if custody {
                ClaimCustody::Retained(cancellation)
            } else {
                ClaimCustody::Released
            },
        );
    }
    if run_cancel.is_cancelled() {
        turn.cancel();
    }
    (
        Some(BoundClaim {
            context,
            ids,
            receipt,
            bound,
            bound_at,
            turn,
            cancellation,
        }),
        ClaimCustody::Released,
    )
}

async fn consume_bound_claim(bound: BoundClaim<'_>) -> ClaimCustody {
    let BoundClaim {
        context,
        ids,
        receipt,
        bound,
        bound_at,
        turn,
        cancellation,
    } = bound;
    let scope = RunBatchScope {
        claimed: &context.claimed,
        launched: &receipt,
        bound: &bound,
        run_start_key: &ids.run_start_key,
        credentials: &ids.credentials,
        expected_launch_at: ids.operated_at,
        expected_updated_at: bound_at,
    };
    let run_cancel = cancellation.cancel_handle();
    let custody_unresolved = consume_turn(
        context.repository,
        context.config,
        context.origin,
        context.stop,
        context.process_cancel,
        run_cancel.as_ref(),
        turn,
        scope,
    )
    .await;
    if custody_unresolved {
        ClaimCustody::Retained(cancellation)
    } else {
        ClaimCustody::Released
    }
}

async fn read_payload(
    repository: &Repository,
    claimed: &ClaimedMessageDispatch,
) -> Option<artisan_database::QueueMessageDispatchPayload> {
    repository
        .read_queue_message_dispatch_payload(&claimed.message_id)
        .await
        .ok()
        .flatten()
}

async fn requeue_claim(
    repository: &Repository,
    claimed: ClaimedMessageDispatch,
    config: &NativeRunDispatcherConfig,
    origin: &SystemCommandOrigin,
    reason: &'static str,
) {
    let Some(operated_at) = wall_clock(origin) else {
        return;
    };
    let Some(available_at) = add_duration(operated_at, config.retry_backoff) else {
        return;
    };
    let Ok(reason) = DispatchFailureReason::parse(reason) else {
        return;
    };
    let _ = repository
        .requeue_message_dispatch(RequeueMessageDispatch {
            message_id: claimed.message_id,
            owner: claimed.owner,
            operated_at,
            available_at,
            reason,
        })
        .await;
}

fn is_permanent_configuration_error(error: &artisan_database::RepositoryError) -> bool {
    matches!(
        error,
        artisan_database::RepositoryError::CorruptData { .. }
            | artisan_database::RepositoryError::Invariant { .. }
            | artisan_database::RepositoryError::ProjectNotFound { .. }
            | artisan_database::RepositoryError::ThreadNotFound { .. }
    )
}

async fn fail_claim(
    repository: &Repository,
    claimed: ClaimedMessageDispatch,
    origin: &SystemCommandOrigin,
    reason: &'static str,
) {
    let Some(operated_at) = wall_clock(origin) else {
        return;
    };
    let Ok(reason) = DispatchFailureReason::parse(reason) else {
        return;
    };
    let _ = repository
        .fail_message_dispatch(FailMessageDispatch {
            message_id: claimed.message_id,
            owner: claimed.owner,
            operated_at,
            reason,
        })
        .await;
}

async fn launch_with_retry(
    repository: &Repository,
    command: LaunchClaimedRun<'_>,
    retries: std::num::NonZeroUsize,
) -> Result<LaunchClaimedRunOutcome, artisan_database::RunLaunchError> {
    let LaunchClaimedRun {
        claimed,
        run_id,
        turn_id,
        item_id,
        first_patch_id,
        second_patch_id,
        operated_at,
        run_start_key,
        credentials,
        engine_settings,
    } = command;
    let mut last_error = None;
    for _ in 0..retries.get() {
        match repository
            .launch_claimed_run(LaunchClaimedRun {
                claimed,
                run_id,
                turn_id,
                item_id,
                first_patch_id,
                second_patch_id,
                operated_at,
                run_start_key,
                credentials,
                engine_settings,
            })
            .await
        {
            Ok(outcome) => return Ok(outcome),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.expect("positive retry count always records a result"))
}

async fn bind_with_retry(
    repository: &Repository,
    command: BindRunProvider<'_>,
    retries: std::num::NonZeroUsize,
) -> Result<BindRunProviderOutcome, artisan_database::RunBindingError> {
    let BindRunProvider {
        claimed,
        receipt,
        run_start_key,
        credentials,
        expected_launch_at,
        bound_at,
        binding_version,
        binding_bytes,
    } = command;
    let mut last_error = None;
    for _ in 0..retries.get() {
        match repository
            .bind_run_provider(BindRunProvider {
                claimed,
                receipt,
                run_start_key,
                credentials,
                expected_launch_at,
                bound_at,
                binding_version,
                binding_bytes,
            })
            .await
        {
            Ok(outcome) => return Ok(outcome),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.expect("positive retry count always records a result"))
}

fn provider_binding_bytes(profile_id: &str, session_id: &str) -> Option<ProviderBindingBytes> {
    let value = serde_json::json!({
        "engine": PROVIDER_BINDING_ENGINE,
        "profile_id": profile_id,
        "session_id": session_id,
    });
    let bytes = serde_json::to_vec(&value).ok()?;
    ProviderBindingBytes::new(bytes).ok()
}

async fn abandon_turn(
    mut turn: AcceptedTurn,
    stop: &CancelHandle,
    process_cancel: &CancelHandle,
) -> bool {
    turn.cancel();
    let _ = drain_turn(&mut turn, stop, process_cancel).await;
    is_unresolved_reap(&turn.finish().await)
}

async fn drain_turn(
    turn: &mut AcceptedTurn,
    stop: &CancelHandle,
    process_cancel: &CancelHandle,
) -> bool {
    if stop.is_cancelled() || process_cancel.is_cancelled() {
        turn.cancel();
    }
    while turn.next_observation().await.is_some() {
        if stop.is_cancelled() || process_cancel.is_cancelled() {
            turn.cancel();
        }
    }
    true
}

struct CommitBatchRequest<'a> {
    repository: &'a Repository,
    notifier: &'a ConversationCommitNotifier,
    scope: &'a RunBatchScope<'a>,
    batch_sequence: i64,
    operated_at: UnixMillis,
    activate_turn_patch_id: Option<&'a PatchId>,
    changes: &'a [AssistantChange<'a>],
    retries: std::num::NonZeroUsize,
}

async fn commit_batch_with_retry(request: CommitBatchRequest<'_>) -> bool {
    let CommitBatchRequest {
        repository,
        notifier,
        scope,
        batch_sequence,
        operated_at,
        activate_turn_patch_id,
        changes,
        retries,
    } = request;
    for _ in 0..retries.get() {
        let result = repository
            .commit_run_batch(CommitRunBatch {
                scope: RunBatchScope {
                    claimed: scope.claimed,
                    launched: scope.launched,
                    bound: scope.bound,
                    run_start_key: scope.run_start_key,
                    credentials: scope.credentials,
                    expected_launch_at: scope.expected_launch_at,
                    expected_updated_at: scope.expected_updated_at,
                },
                batch_sequence,
                operated_at,
                activate_turn_patch_id,
                changes,
                checkpoint: artisan_database::CheckpointUpdate::Keep,
            })
            .await;
        if notify_after_commit(
            matches!(
                result,
                Ok(CommitRunBatchOutcome::Committed(_)
                    | CommitRunBatchOutcome::AlreadyCommitted(_))
            ),
            || {
                let _ = notifier.publish(&scope.launched.thread_id);
            },
        ) {
            return true;
        }
    }
    false
}

fn copy_scope<'a>(scope: &RunBatchScope<'a>) -> RunBatchScope<'a> {
    RunBatchScope {
        claimed: scope.claimed,
        launched: scope.launched,
        bound: scope.bound,
        run_start_key: scope.run_start_key,
        credentials: scope.credentials,
        expected_launch_at: scope.expected_launch_at,
        expected_updated_at: scope.expected_updated_at,
    }
}

struct TurnConsumptionContext<'a> {
    repository: &'a Repository,
    config: &'a NativeRunDispatcherConfig,
    origin: &'a SystemCommandOrigin,
    stop: &'a CancelHandle,
    process_cancel: &'a CancelHandle,
    run_cancel: &'a CancelHandle,
}

struct TurnConsumptionState<'a> {
    scope: RunBatchScope<'a>,
    assistant_item: Option<ItemId>,
    assistant_revision: Revision,
    assistant_parts: OrderedAssistantText,
    assistant_body: String,
    batch_sequence: i64,
    forced_interrupted: bool,
    forced_cancelled: bool,
    progress_uncertain: bool,
    terminal: Option<TerminalState>,
}

impl<'a> TurnConsumptionState<'a> {
    fn new(scope: RunBatchScope<'a>) -> Self {
        Self {
            scope,
            assistant_item: None,
            assistant_revision: Revision::new(0),
            assistant_parts: OrderedAssistantText::default(),
            assistant_body: String::new(),
            batch_sequence: 1,
            forced_interrupted: false,
            forced_cancelled: false,
            progress_uncertain: false,
            terminal: None,
        }
    }
}

async fn consume_turn(
    repository: &Repository,
    config: &NativeRunDispatcherConfig,
    origin: &SystemCommandOrigin,
    stop: &CancelHandle,
    process_cancel: &CancelHandle,
    run_cancel: &CancelHandle,
    mut turn: crate::engine_owner::operation::AcceptedTurn,
    scope: RunBatchScope<'_>,
) -> bool {
    let context = TurnConsumptionContext {
        repository,
        config,
        origin,
        stop,
        process_cancel,
        run_cancel,
    };
    let mut state = TurnConsumptionState::new(scope);

    let mut cancel_signalled = false;
    loop {
        if cancel_signalled {
            // Cancellation was already delivered to the turn: drain the
            // remaining observations to the driver's terminal close. The
            // fired cancel branches stay ready forever once cancelled, so
            // re-selecting them under `biased` would starve
            // `next_observation()` on a held stream that never emits a
            // terminal event.
            let observation = turn.next_observation().await;
            let Some(observation) = observation else {
                break;
            };
            handle_observation(&context, &mut state, &mut turn, observation).await;
        } else {
            tokio::select! {
                biased;
                () = context.stop.wait() => {
                    cancel_signalled = true;
                    state.forced_interrupted = true;
                    turn.cancel();
                }
                () = context.process_cancel.wait() => {
                    cancel_signalled = true;
                    state.forced_interrupted = true;
                    turn.cancel();
                }
                () = context.run_cancel.wait() => {
                    cancel_signalled = true;
                    state.forced_cancelled = true;
                    turn.cancel();
                }
                observation = turn.next_observation() => {
                    let Some(observation) = observation else { break; };
                    handle_observation(&context, &mut state, &mut turn, observation).await;
                }
            }
        }
        if state.terminal.is_some() {
            break;
        }
    }
    let owner_result = turn.finish().await;
    if is_unresolved_reap(&owner_result) {
        return true;
    }
    if state.progress_uncertain {
        return false;
    }
    if context.run_cancel.is_cancelled() {
        state.forced_cancelled = true;
    }
    let Some(terminal) = resolve_terminal(
        state.forced_interrupted,
        state.forced_cancelled,
        state.terminal,
        &owner_result,
    ) else {
        return false;
    };
    if !ensure_assistant_item(&context, &mut state).await {
        return false;
    }
    settle_terminal(&context, state, terminal).await;
    false
}

fn mark_interrupted(
    state: &mut TurnConsumptionState<'_>,
    turn: &AcceptedTurn,
    progress_uncertain: bool,
) {
    state.forced_interrupted = true;
    state.progress_uncertain |= progress_uncertain;
    turn.cancel();
}

async fn handle_observation(
    context: &TurnConsumptionContext<'_>,
    state: &mut TurnConsumptionState<'_>,
    turn: &mut AcceptedTurn,
    observation: EngineObservation,
) {
    match observation {
        EngineObservation::TextDelta(delta) => {
            handle_text_delta(context, state, turn, delta).await;
        }
        EngineObservation::TextSnapshot(snapshot) => {
            handle_text_snapshot(context, state, turn, snapshot).await;
        }
        EngineObservation::Usage(usage) => {
            handle_usage(context, state, turn, usage).await;
        }
        EngineObservation::Terminal(observation) => {
            state.terminal = Some(observation.state());
            if state.forced_interrupted || state.forced_cancelled {
                turn.cancel();
            }
        }
    }
}

async fn handle_usage(
    context: &TurnConsumptionContext<'_>,
    state: &mut TurnConsumptionState<'_>,
    turn: &mut AcceptedTurn,
    usage: UsageObservation,
) {
    let report = usage.report();
    if report.run_id() != &state.scope.launched.run_id
        || report.thread_id() != &state.scope.launched.thread_id
        || context
            .repository
            .record_run_usage(RecordRunUsage {
                run_id: &state.scope.launched.run_id,
                thread_id: &state.scope.launched.thread_id,
                report,
            })
            .await
            .is_err()
    {
        // A valid text stream must not be presented as durably completed when
        // its authenticated usage observation could not be fenced/persisted.
        mark_interrupted(state, turn, true);
    }
}

async fn handle_text_snapshot(
    context: &TurnConsumptionContext<'_>,
    state: &mut TurnConsumptionState<'_>,
    turn: &mut AcceptedTurn,
    snapshot: TextSnapshot,
) {
    if snapshot.run_id() != &state.scope.launched.run_id {
        mark_interrupted(state, turn, true);
        return;
    }
    let current = state.assistant_body.clone();
    let Some(next_body) = state.assistant_parts.replace_snapshot(&snapshot) else {
        mark_interrupted(state, turn, true);
        return;
    };
    if next_body == current {
        return;
    }
    state.assistant_body = next_body;
    if state.assistant_item.is_none() {
        // An ended-only part is a first durable body, not a text delta. Start
        // it through the normal run-scoped item-upsert seam without replaying
        // an artificial provider append.
        start_assistant_item(context, state, turn).await;
    } else {
        replace_assistant_body(context, state, turn).await;
    }
}

async fn handle_text_delta(
    context: &TurnConsumptionContext<'_>,
    state: &mut TurnConsumptionState<'_>,
    turn: &mut AcceptedTurn,
    delta: TextDelta,
) {
    if delta.run_id() != &state.scope.launched.run_id || delta.delta().is_empty() {
        if delta.run_id() != &state.scope.launched.run_id {
            mark_interrupted(state, turn, true);
        }
        return;
    }
    let previous = state.assistant_body.clone();
    let Some(next_body) = state.assistant_parts.append_delta(&delta) else {
        mark_interrupted(state, turn, true);
        return;
    };
    state.assistant_body = next_body.clone();
    if state.assistant_item.is_none() {
        start_assistant_item(context, state, turn).await;
    } else {
        let mut append_body = previous;
        append_body.push_str(delta.delta());
        if append_body == next_body {
            append_assistant_delta(context, state, turn, delta).await;
        } else {
            replace_assistant_body(context, state, turn).await;
        }
    }
}

async fn replace_assistant_body(
    context: &TurnConsumptionContext<'_>,
    state: &mut TurnConsumptionState<'_>,
    turn: &mut AcceptedTurn,
) {
    let Ok(body) = AssistantBody::parse(state.assistant_body.clone()) else {
        mark_interrupted(state, turn, true);
        return;
    };
    let Some(item_id) = state.assistant_item.clone() else {
        mark_interrupted(state, turn, true);
        return;
    };
    let Ok(next_revision) = state.assistant_revision.checked_next() else {
        mark_interrupted(state, turn, true);
        return;
    };
    let Some(patch_id) = mint_patch_id(context.origin) else {
        mark_interrupted(state, turn, false);
        return;
    };
    let changes = [AssistantChange::Replace {
        item_id: &item_id,
        expected_revision: state.assistant_revision,
        body: &body,
        phase: AssistantMessagePhase::Unspecified,
        patch_id: &patch_id,
    }];
    let Some(operated_at) = at_or_after(context.origin, state.scope.expected_updated_at) else {
        mark_interrupted(state, turn, false);
        return;
    };
    if !commit_batch_with_retry(CommitBatchRequest {
        repository: context.repository,
        notifier: &context.config.notifier,
        scope: &state.scope,
        batch_sequence: state.batch_sequence,
        operated_at,
        activate_turn_patch_id: None,
        changes: &changes,
        retries: context.config.max_command_retries,
    })
    .await
    {
        mark_interrupted(state, turn, true);
        return;
    }
    state.assistant_revision = next_revision;
    state.scope.expected_updated_at = operated_at;
    let Some(next_sequence) = state.batch_sequence.checked_add(1) else {
        mark_interrupted(state, turn, true);
        return;
    };
    state.batch_sequence = next_sequence;
}

async fn start_assistant_item(
    context: &TurnConsumptionContext<'_>,
    state: &mut TurnConsumptionState<'_>,
    turn: &mut AcceptedTurn,
) {
    let Some(item_id) = mint_item_id(context.origin) else {
        mark_interrupted(state, turn, false);
        return;
    };
    let Ok(body) = AssistantBody::parse(state.assistant_body.clone()) else {
        mark_interrupted(state, turn, false);
        return;
    };
    let Some(patch_id) = mint_patch_id(context.origin) else {
        mark_interrupted(state, turn, false);
        return;
    };
    let Some(activation_patch_id) = mint_patch_id(context.origin) else {
        mark_interrupted(state, turn, false);
        return;
    };
    let changes = [AssistantChange::Start {
        item_id: &item_id,
        phase: AssistantMessagePhase::Unspecified,
        body: &body,
        patch_id: &patch_id,
    }];
    let Some(operated_at) = at_or_after(context.origin, state.scope.expected_updated_at) else {
        mark_interrupted(state, turn, false);
        return;
    };
    if !commit_batch_with_retry(CommitBatchRequest {
        repository: context.repository,
        notifier: &context.config.notifier,
        scope: &state.scope,
        batch_sequence: state.batch_sequence,
        operated_at,
        activate_turn_patch_id: Some(&activation_patch_id),
        changes: &changes,
        retries: context.config.max_command_retries,
    })
    .await
    {
        mark_interrupted(state, turn, true);
        return;
    }
    state.assistant_item = Some(item_id);
    state.assistant_revision = Revision::new(0);
    state.scope.expected_updated_at = operated_at;
    let Some(next_sequence) = state.batch_sequence.checked_add(1) else {
        mark_interrupted(state, turn, true);
        return;
    };
    state.batch_sequence = next_sequence;
}

async fn append_assistant_delta(
    context: &TurnConsumptionContext<'_>,
    state: &mut TurnConsumptionState<'_>,
    turn: &mut AcceptedTurn,
    delta: TextDelta,
) {
    let Some(item_id) = state.assistant_item.as_ref() else {
        mark_interrupted(state, turn, true);
        return;
    };
    let Ok(fragment) = IncrementalText::parse(delta.delta().to_owned()) else {
        mark_interrupted(state, turn, false);
        return;
    };
    let Some(patch_id) = mint_patch_id(context.origin) else {
        mark_interrupted(state, turn, false);
        return;
    };
    let changes = [AssistantChange::Append {
        item_id,
        expected_revision: state.assistant_revision,
        text: &fragment,
        patch_id: &patch_id,
    }];
    let Some(operated_at) = at_or_after(context.origin, state.scope.expected_updated_at) else {
        mark_interrupted(state, turn, false);
        return;
    };
    if !commit_batch_with_retry(CommitBatchRequest {
        repository: context.repository,
        notifier: &context.config.notifier,
        scope: &state.scope,
        batch_sequence: state.batch_sequence,
        operated_at,
        activate_turn_patch_id: None,
        changes: &changes,
        retries: context.config.max_command_retries,
    })
    .await
    {
        mark_interrupted(state, turn, true);
        return;
    }
    let Ok(next_revision) = state.assistant_revision.checked_next() else {
        mark_interrupted(state, turn, true);
        return;
    };
    state.assistant_revision = next_revision;
    state.scope.expected_updated_at = operated_at;
    let Some(next_sequence) = state.batch_sequence.checked_add(1) else {
        mark_interrupted(state, turn, true);
        return;
    };
    state.batch_sequence = next_sequence;
}

fn resolve_terminal(
    forced_interrupted: bool,
    forced_cancelled: bool,
    terminal: Option<TerminalState>,
    owner_result: &TurnResult,
) -> Option<TerminalState> {
    if forced_interrupted {
        return Some(TerminalState::Interrupted);
    }
    if forced_cancelled {
        return Some(TerminalState::Cancelled);
    }
    if let Some(terminal) = terminal {
        return Some(terminal);
    }
    match owner_result {
        Ok(result) => Some(result.terminal()),
        Err(EngineOperationError::Cancelled) => Some(TerminalState::Cancelled),
        Err(EngineOperationError::Shutdown | EngineOperationError::Deadline) => {
            Some(TerminalState::Interrupted)
        }
        Err(
            EngineOperationError::ReapUnresolved
            | EngineOperationError::UnresolvedReapDuring { .. },
        ) => None,
        Err(_) => Some(TerminalState::Failed),
    }
}

fn is_unresolved_reap(result: &TurnResult) -> bool {
    matches!(
        result,
        Err(EngineOperationError::ReapUnresolved
            | EngineOperationError::UnresolvedReapDuring { .. })
    )
}

async fn ensure_assistant_item(
    context: &TurnConsumptionContext<'_>,
    state: &mut TurnConsumptionState<'_>,
) -> bool {
    if state.assistant_item.is_some() {
        return true;
    }
    let Some(item_id) = mint_item_id(context.origin) else {
        return false;
    };
    let Ok(body) = AssistantBody::parse(state.assistant_body.clone()) else {
        return false;
    };
    let Some(patch_id) = mint_patch_id(context.origin) else {
        return false;
    };
    let Some(activation_patch_id) = mint_patch_id(context.origin) else {
        return false;
    };
    let changes = [AssistantChange::Start {
        item_id: &item_id,
        phase: AssistantMessagePhase::Unspecified,
        body: &body,
        patch_id: &patch_id,
    }];
    let Some(operated_at) = at_or_after(context.origin, state.scope.expected_updated_at) else {
        return false;
    };
    if !commit_batch_with_retry(CommitBatchRequest {
        repository: context.repository,
        notifier: &context.config.notifier,
        scope: &state.scope,
        batch_sequence: state.batch_sequence,
        operated_at,
        activate_turn_patch_id: Some(&activation_patch_id),
        changes: &changes,
        retries: context.config.max_command_retries,
    })
    .await
    {
        return false;
    }
    state.assistant_item = Some(item_id);
    state.assistant_revision = Revision::new(0);
    state.scope.expected_updated_at = operated_at;
    true
}

struct TerminalSettlement<'a> {
    repository: &'a Repository,
    scope: &'a RunBatchScope<'a>,
    retries: std::num::NonZeroUsize,
    operated_at: UnixMillis,
    item_id: &'a ItemId,
    expected_revision: Revision,
    body: &'a AssistantBody,
    phase: AssistantMessagePhase,
    item_patch_id: &'a PatchId,
    turn_patch_id: &'a PatchId,
}

async fn settle_terminal(
    context: &TurnConsumptionContext<'_>,
    state: TurnConsumptionState<'_>,
    terminal: TerminalState,
) {
    let TurnConsumptionState {
        scope,
        assistant_item: Some(item_id),
        assistant_revision,
        assistant_body,
        ..
    } = state
    else {
        return;
    };
    let Ok(body) = AssistantBody::parse(assistant_body) else {
        return;
    };
    let Some(item_patch_id) = mint_patch_id(context.origin) else {
        return;
    };
    let Some(turn_patch_id) = mint_patch_id(context.origin) else {
        return;
    };
    let Some(operated_at) = at_or_after(context.origin, scope.expected_updated_at) else {
        return;
    };
    let phase = if matches!(terminal, TerminalState::Completed) {
        AssistantMessagePhase::Final
    } else {
        AssistantMessagePhase::Unspecified
    };
    let settlement = TerminalSettlement {
        repository: context.repository,
        scope: &scope,
        retries: context.config.max_command_retries,
        operated_at,
        item_id: &item_id,
        expected_revision: assistant_revision,
        body: &body,
        phase,
        item_patch_id: &item_patch_id,
        turn_patch_id: &turn_patch_id,
    };
    if persist_terminal_settlement(&settlement, terminal).await {
        let _ = context.config.notifier.publish(&scope.launched.thread_id);
    }
}

async fn persist_terminal_settlement(
    settlement: &TerminalSettlement<'_>,
    terminal: TerminalState,
) -> bool {
    match terminal {
        TerminalState::Completed => persist_completed(settlement).await,
        TerminalState::Failed => persist_failed(settlement).await,
        TerminalState::Cancelled => persist_cancelled(settlement).await,
        TerminalState::Interrupted => persist_interrupted(settlement).await,
    }
}

async fn persist_completed(settlement: &TerminalSettlement<'_>) -> bool {
    for _ in 0..settlement.retries.get() {
        if settlement
            .repository
            .complete_run(CompleteRun {
                scope: copy_scope(settlement.scope),
                operated_at: settlement.operated_at,
                item_id: settlement.item_id,
                expected_revision: settlement.expected_revision,
                body: settlement.body,
                phase: settlement.phase,
                item_patch_id: settlement.item_patch_id,
                turn_patch_id: settlement.turn_patch_id,
            })
            .await
            .is_ok()
        {
            return true;
        }
    }
    false
}

async fn persist_failed(settlement: &TerminalSettlement<'_>) -> bool {
    let Ok(error_code) = RunErrorCode::parse(PROVIDER_FAILURE_CODE.to_owned()) else {
        return false;
    };
    let Ok(error_message) = RunErrorMessage::parse(PROVIDER_FAILURE_MESSAGE.to_owned()) else {
        return false;
    };
    for _ in 0..settlement.retries.get() {
        if settlement
            .repository
            .fail_run(artisan_database::FailRun {
                scope: copy_scope(settlement.scope),
                operated_at: settlement.operated_at,
                item_id: settlement.item_id,
                expected_revision: settlement.expected_revision,
                body: settlement.body,
                phase: settlement.phase,
                item_patch_id: settlement.item_patch_id,
                turn_patch_id: settlement.turn_patch_id,
                error_code: &error_code,
                error_message: &error_message,
            })
            .await
            .is_ok()
        {
            return true;
        }
    }
    false
}

async fn persist_cancelled(settlement: &TerminalSettlement<'_>) -> bool {
    for _ in 0..settlement.retries.get() {
        if settlement
            .repository
            .cancel_run(artisan_database::CancelRun {
                scope: copy_scope(settlement.scope),
                operated_at: settlement.operated_at,
                item_id: settlement.item_id,
                expected_revision: settlement.expected_revision,
                body: settlement.body,
                phase: settlement.phase,
                item_patch_id: settlement.item_patch_id,
                turn_patch_id: settlement.turn_patch_id,
            })
            .await
            .is_ok()
        {
            return true;
        }
    }
    false
}

async fn persist_interrupted(settlement: &TerminalSettlement<'_>) -> bool {
    let Ok(error_code) = RunErrorCode::parse(INTERRUPTED_CODE.to_owned()) else {
        return false;
    };
    let Ok(error_message) = RunErrorMessage::parse(INTERRUPTED_MESSAGE.to_owned()) else {
        return false;
    };
    for _ in 0..settlement.retries.get() {
        if settlement
            .repository
            .interrupt_run(InterruptRun {
                scope: copy_scope(settlement.scope),
                operated_at: settlement.operated_at,
                item_id: settlement.item_id,
                expected_revision: settlement.expected_revision,
                body: settlement.body,
                phase: settlement.phase,
                item_patch_id: settlement.item_patch_id,
                turn_patch_id: settlement.turn_patch_id,
                error_code: &error_code,
                error_message: &error_message,
            })
            .await
            .is_ok()
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod text_projection_tests {
    use artisan_domain::RunId;

    use super::OrderedAssistantText;
    use crate::engine_owner::observation::{TextSnapshot, chunk_text};

    #[test]
    fn multipart_byte_bound_tracks_all_parts_and_replacements() {
        let mut parts = OrderedAssistantText::default();
        let full = "x".repeat(artisan_domain::AssistantBody::MAX_BYTES - 1);
        assert!(parts.append("a", &full).is_some());
        assert!(parts.append("b", "b").is_some());
        assert!(parts.append("c", "c").is_none());
        assert!(parts.replace("b", "bb").is_none());
        assert!(parts.replace("a", "a").is_some());
        assert_eq!(parts.append("c", "c").as_deref(), Some("abc"));
        assert_eq!(parts.total_bytes, 3);
    }

    #[test]
    fn correcting_second_text_part_retains_first_without_duplicate_bytes() {
        let run_id = RunId::parse("text-projection-run").expect("bounded run id");
        let mut parts = OrderedAssistantText::default();
        let part_a_delta = chunk_text(&run_id, 1, "event-a", "A-1")
            .pop()
            .expect("part A delta")
            .with_part_id("part-a".to_owned());
        assert_eq!(parts.append_delta(&part_a_delta), Some("A-1".to_owned()));
        let part_a_delta = chunk_text(&run_id, 2, "event-a-2", "A-2")
            .pop()
            .expect("part A second delta")
            .with_part_id("part-a".to_owned());
        assert_eq!(parts.append_delta(&part_a_delta), Some("A-1A-2".to_owned()));
        assert_eq!(
            parts.replace_snapshot(&TextSnapshot::new(
                run_id.clone(),
                3,
                "part-a".to_owned(),
                "A-1A-2".to_owned(),
            )),
            Some("A-1A-2".to_owned())
        );
        let part_b_delta = chunk_text(&run_id, 4, "event-b", "B-1")
            .pop()
            .expect("part B delta")
            .with_part_id("part-b".to_owned());
        assert_eq!(
            parts.append_delta(&part_b_delta),
            Some("A-1A-2B-1".to_owned())
        );
        assert_eq!(
            parts.replace_snapshot(&TextSnapshot::new(
                run_id,
                5,
                "part-b".to_owned(),
                "B-corrected".to_owned(),
            )),
            Some("A-1A-2B-corrected".to_owned())
        );
        assert_eq!(parts.body(), "A-1A-2B-corrected");
    }
}
