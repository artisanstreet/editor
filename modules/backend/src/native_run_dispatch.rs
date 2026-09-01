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
    LaunchClaimedRunOutcome, LaunchedRunReceipt, ProviderBindingBytes, Repository,
    RequeueMessageDispatch, RunBatchScope, RunErrorCode, RunErrorMessage, RunLaunchCredentials,
    RunLaunchError, RunStartKey,
};
use artisan_domain::{
    AssistantBody, AssistantMessagePhase, IncrementalText, ItemId, PatchId, Revision, RootPath,
    RunId, TurnId, UnixMillis,
};
use artisan_native_engine::{NativeOpenCode2Authority, VerifiedOpenCode2ProfileLaunch};
use artisan_transport::CancelHandle;
use tokio::{runtime::Handle, task::JoinHandle};

use crate::{
    CommandOrigin, SystemCommandOrigin,
    conversation_commit_notifier::ConversationCommitNotifier,
    engine_owner::EngineTurnInput,
    engine_owner::observation::{EngineObservation, TerminalState, TextDelta},
    engine_owner::operation::{AcceptedTurn, EngineOperationError, PreparedSession, TurnResult},
    engine_owner::{EngineOwner, EngineOwnerShutdown},
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

/// One running configured first-message dispatcher.
#[allow(clippy::module_name_repetitions)]
pub struct NativeRunDispatcher {
    stop: Arc<CancelHandle>,
    shutdown_budget: Duration,
    join: Option<JoinHandle<DispatchLoopExit>>,
    observed: Option<NativeRunDispatcherShutdown>,
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
        runtime: &Handle,
    ) -> Self {
        let shutdown_budget = config.shutdown_budget;
        let stop = Arc::new(CancelHandle::new());
        let owner = EngineOwner::start_configured(config.queue_capacity, runtime);
        let join = runtime.spawn(dispatch_loop(
            repository,
            database_path,
            config,
            Arc::clone(&stop),
            process_cancel,
            owner,
        ));
        Self {
            stop,
            shutdown_budget,
            join: Some(join),
            observed: None,
        }
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

async fn dispatch_loop(
    repository: Repository,
    database_path: PathBuf,
    config: NativeRunDispatcherConfig,
    stop: Arc<CancelHandle>,
    process_cancel: Arc<CancelHandle>,
    mut owner: EngineOwner,
) -> DispatchLoopExit {
    let origin = SystemCommandOrigin;
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
        let Some(claimed_at) = wall_clock(&origin) else {
            if !wait_for_next_claim(&stop, &process_cancel, config.poll_interval).await {
                break;
            }
            continue;
        };
        let Some(lease_expires_at) = add_duration(claimed_at, config.claim_lease) else {
            if !wait_for_next_claim(&stop, &process_cancel, config.poll_interval).await {
                break;
            }
            continue;
        };
        let Some(owner_token) = mint_dispatch_owner() else {
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
            if !wait_for_next_claim(&stop, &process_cancel, config.poll_interval).await {
                break;
            }
            continue;
        };
        execute_claim(ClaimExecution {
            repository: &repository,
            database_path: Path::new(&database_path),
            config: &config,
            origin: &origin,
            stop: &stop,
            process_cancel: &process_cancel,
            owner: &owner,
            claimed,
        })
        .await;
    }

    run_final_recovery_page(&repository, &config, &origin).await;

    let owner_shutdown = loop {
        let outcome = owner.shutdown().await;
        if !matches!(outcome, EngineOwnerShutdown::Quarantined) {
            break outcome;
        }
    };
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
    payload: artisan_database::MessageDispatchPayload,
    settings: artisan_database::ThreadEngineSettings,
    project_root: RootPath,
    launch: VerifiedOpenCode2ProfileLaunch,
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
    payload: artisan_database::MessageDispatchPayload,
    settings: artisan_database::ThreadEngineSettings,
    project_root: RootPath,
    launch: VerifiedOpenCode2ProfileLaunch,
    ids: ClaimIds,
    receipt: LaunchedRunReceipt,
}

struct PreparedClaim<'a> {
    context: ClaimExecution<'a>,
    ids: ClaimIds,
    receipt: LaunchedRunReceipt,
    settings: artisan_database::ThreadEngineSettings,
    turn: AcceptedTurn,
    session: PreparedSession,
}

struct BoundClaim<'a> {
    context: ClaimExecution<'a>,
    ids: ClaimIds,
    receipt: LaunchedRunReceipt,
    bound: artisan_database::BoundRunReceipt,
    bound_at: UnixMillis,
    turn: AcceptedTurn,
}

async fn execute_claim(context: ClaimExecution<'_>) {
    if context.stop.is_cancelled() || context.process_cancel.is_cancelled() {
        context.requeue("dispatcher stopping").await;
        return;
    }
    let Some(loaded) = load_claim(context).await else {
        return;
    };
    let ids = match mint_claim_ids(loaded.context.origin, loaded.context.claimed.updated_at) {
        Ok(ids) => ids,
        Err(reason) => {
            loaded.context.requeue(reason).await;
            return;
        }
    };
    let Some(launched) = launch_claim(loaded, ids).await else {
        return;
    };
    let Some(prepared) = admit_claim(launched).await else {
        return;
    };
    let Some(bound) = bind_claim(prepared).await else {
        return;
    };
    consume_bound_claim(bound).await;
}

async fn load_claim(context: ClaimExecution<'_>) -> Option<LoadedClaim<'_>> {
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
    let profile_id = settings.config().selection().as_opencode2().profile_id();
    let Ok(launch) = context
        .config
        .authority
        .resolve_profile_launch(context.database_path, profile_id)
    else {
        context.requeue("engine profile unavailable").await;
        return None;
    };
    Some(LoadedClaim {
        context,
        payload,
        settings,
        project_root,
        launch,
    })
}

fn mint_claim_ids(
    origin: &SystemCommandOrigin,
    updated_at: UnixMillis,
) -> Result<ClaimIds, &'static str> {
    let run_id = mint_run_id(origin).ok_or("run identity unavailable")?;
    let turn_id = mint_turn_id(origin).ok_or("run identity unavailable")?;
    let item_id = mint_item_id(origin).ok_or("run identity unavailable")?;
    let first_patch_id = mint_patch_id(origin).ok_or("run identity unavailable")?;
    let second_patch_id = mint_patch_id(origin).ok_or("run identity unavailable")?;
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

async fn launch_claim(loaded: LoadedClaim<'_>, ids: ClaimIds) -> Option<LaunchedClaim<'_>> {
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
        ids,
        receipt,
    })
}

async fn admit_claim(claim: LaunchedClaim<'_>) -> Option<PreparedClaim<'_>> {
    let LaunchedClaim {
        context,
        payload,
        settings,
        project_root,
        launch,
        ids,
        receipt,
    } = claim;
    let attempt_budget = Duration::from_millis(settings.config().runtime().attempt_budget().get());
    let input = EngineTurnInput {
        run_id: receipt.run_id.clone(),
        project_root,
        prompt_id: payload.message_id.as_str().to_owned(),
        prompt_text: payload.body,
        settings: settings.clone(),
        launch,
        prompt_delivery: context.config.prompt_delivery.clone(),
        stream_after: context.config.stream_after,
        control_capacity: context.config.queue_capacity.get(),
    };
    let Ok(mut turn) = context.owner.admit_turn(input, attempt_budget) else {
        return None;
    };
    let Ok(session) = turn.prepare().await else {
        let _ = turn.finish().await;
        return None;
    };
    Some(PreparedClaim {
        context,
        ids,
        receipt,
        settings,
        turn,
        session,
    })
}

async fn bind_claim(claim: PreparedClaim<'_>) -> Option<BoundClaim<'_>> {
    let PreparedClaim {
        context,
        ids,
        receipt,
        settings,
        mut turn,
        session,
    } = claim;
    let Some(binding_bytes) = provider_binding_bytes(
        settings
            .config()
            .selection()
            .as_opencode2()
            .profile_id()
            .as_str(),
        session.session(),
    ) else {
        abandon_turn(turn, context.stop, context.process_cancel).await;
        return None;
    };
    let Some(bound_at) = at_or_after(context.origin, ids.operated_at) else {
        abandon_turn(turn, context.stop, context.process_cancel).await;
        return None;
    };
    let bind_result = bind_with_retry(
        context.repository,
        BindRunProvider {
            claimed: &context.claimed,
            receipt: &receipt,
            run_start_key: &ids.run_start_key,
            credentials: &ids.credentials,
            expected_launch_at: ids.operated_at,
            bound_at,
            binding_version: PROVIDER_BINDING_VERSION,
            binding_bytes: &binding_bytes,
        },
        context.config.max_command_retries,
    )
    .await;
    let (bound, already_bound) = match bind_result {
        Ok(BindRunProviderOutcome::Bound(receipt)) => (receipt, false),
        Ok(BindRunProviderOutcome::AlreadyBound(receipt)) => (receipt, true),
        Err(_) => {
            abandon_turn(turn, context.stop, context.process_cancel).await;
            return None;
        }
    };
    if matches!(
        prompt_authorization_after_binding(already_bound),
        PromptAuthorization::DoNotAuthorize
    ) || turn.authorize().is_err()
    {
        abandon_turn(turn, context.stop, context.process_cancel).await;
        return None;
    }
    Some(BoundClaim {
        context,
        ids,
        receipt,
        bound,
        bound_at,
        turn,
    })
}

async fn consume_bound_claim(bound: BoundClaim<'_>) {
    let BoundClaim {
        context,
        ids,
        receipt,
        bound,
        bound_at,
        turn,
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
    consume_turn(
        context.repository,
        context.config,
        context.origin,
        context.stop,
        context.process_cancel,
        turn,
        scope,
    )
    .await;
}

async fn read_payload(
    repository: &Repository,
    claimed: &ClaimedMessageDispatch,
) -> Option<artisan_database::MessageDispatchPayload> {
    repository
        .read_message_dispatch_payload(&claimed.message_id)
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

async fn abandon_turn(mut turn: AcceptedTurn, stop: &CancelHandle, process_cancel: &CancelHandle) {
    turn.cancel();
    let _ = drain_turn(&mut turn, stop, process_cancel).await;
    let _ = turn.finish().await;
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
}

struct TurnConsumptionState<'a> {
    scope: RunBatchScope<'a>,
    assistant_item: Option<ItemId>,
    assistant_revision: Revision,
    assistant_body: String,
    batch_sequence: i64,
    forced_interrupted: bool,
    progress_uncertain: bool,
    terminal: Option<TerminalState>,
}

impl<'a> TurnConsumptionState<'a> {
    fn new(scope: RunBatchScope<'a>) -> Self {
        Self {
            scope,
            assistant_item: None,
            assistant_revision: Revision::new(0),
            assistant_body: String::new(),
            batch_sequence: 1,
            forced_interrupted: false,
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
    mut turn: crate::engine_owner::operation::AcceptedTurn,
    scope: RunBatchScope<'_>,
) {
    let context = TurnConsumptionContext {
        repository,
        config,
        origin,
        stop,
        process_cancel,
    };
    let mut state = TurnConsumptionState::new(scope);

    loop {
        tokio::select! {
            biased;
            () = context.stop.wait() => {
                state.forced_interrupted = true;
                turn.cancel();
            }
            () = context.process_cancel.wait() => {
                state.forced_interrupted = true;
                turn.cancel();
            }
            observation = turn.next_observation() => {
                let Some(observation) = observation else { break; };
                handle_observation(&context, &mut state, &mut turn, observation).await;
            }
        }
        if state.terminal.is_some() {
            break;
        }
    }
    let owner_result = turn.finish().await;
    if state.progress_uncertain {
        return;
    }
    let Some(terminal) = resolve_terminal(state.forced_interrupted, state.terminal, &owner_result)
    else {
        return;
    };
    if !ensure_assistant_item(&context, &mut state).await {
        return;
    }
    settle_terminal(&context, state, terminal).await;
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
        EngineObservation::Terminal(observation) => {
            state.terminal = Some(observation.state());
            if state.forced_interrupted {
                turn.cancel();
            }
        }
    }
}

async fn handle_text_delta(
    context: &TurnConsumptionContext<'_>,
    state: &mut TurnConsumptionState<'_>,
    turn: &mut AcceptedTurn,
    delta: TextDelta,
) {
    let Some(next_length) = state.assistant_body.len().checked_add(delta.delta().len()) else {
        mark_interrupted(state, turn, true);
        return;
    };
    if next_length > AssistantBody::MAX_BYTES {
        mark_interrupted(state, turn, true);
        return;
    }
    state.assistant_body.push_str(delta.delta());
    if state.assistant_item.is_none() {
        start_assistant_item(context, state, turn).await;
    } else {
        append_assistant_delta(context, state, turn, delta).await;
    }
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
    terminal: Option<TerminalState>,
    owner_result: &TurnResult,
) -> Option<TerminalState> {
    if forced_interrupted {
        return Some(TerminalState::Interrupted);
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
