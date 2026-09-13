//! Durable execution for configured native engine profiles.
//!
//! Claims queued messages and authorizes prompts only after binding provider
//! sessions. Persists bounded observations before notifying subscribers.

#![forbid(unsafe_code)]

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use artisan_database::{
    ClaimMessageDispatch, ClaimedMessageDispatch, LaunchedRunReceipt, Repository,
    RunLaunchCredentials, RunStartKey,
};
use artisan_domain::{EngineId, ItemId, PatchId, RootPath, RunId, TurnId, UnixMillis};
use artisan_native_engine::{
    NativeOpenCode2Authority, VerifiedClaudeLaunch, VerifiedCodexLaunch,
    VerifiedOpenCode2ProfileLaunch,
};
use artisan_transport::CancelHandle;
use tokio::{runtime::Handle, task::JoinHandle};

#[cfg(test)]
use crate::engine_owner::FixtureConfiguredLaunch;
use crate::{
    SystemCommandOrigin,
    conversation_commit_notifier::ConversationCommitNotifier,
    engine_owner::EngineContinuation,
    engine_owner::cursor::CursorLaunch,
    engine_owner::grok::GrokLaunch,
    engine_owner::operation::{AcceptedTurn, PreparedSession},
    engine_owner::{EngineOwner, EngineOwnerShutdown},
    lifecycle_control::{ActivityGateError, ActivityGateImpl, ActivityLease},
    run_cancellation::{RunCancellationLease, RunCancellationRegistry},
    run_interaction::RunInteractionRegistry,
};

#[path = "native_run_dispatch/assistant_commit.rs"]
mod assistant_commit;
#[path = "native_run_dispatch/claim.rs"]
mod claim;
#[path = "native_run_dispatch/commit_retry.rs"]
mod commit_retry;
#[path = "native_run_dispatch/delta_coalescer.rs"]
mod delta_coalescer;
#[path = "native_run_dispatch/dispatch_policy.rs"]
mod dispatch_policy;
#[path = "native_run_dispatch/dispatch_support.rs"]
mod dispatch_support;
#[path = "native_run_dispatch/interaction_intent.rs"]
mod interaction_intent;
mod message_parts;
#[path = "native_run_dispatch/observation_commit.rs"]
mod observation_commit;
#[path = "native_run_dispatch/recovery.rs"]
mod recovery;
#[path = "native_run_dispatch/steer.rs"]
mod steer;
mod streaming_speed;
#[path = "native_run_dispatch/text_projection.rs"]
mod text_projection;
#[path = "native_run_dispatch/turn.rs"]
mod turn;

#[cfg(test)]
use assistant_commit::flush_pending_deltas;
#[cfg(test)]
use claim::launch_claim;
use claim::{execute_claim, fail_claim, requeue_claim};
pub(crate) use commit_retry::{CommitBatchRequest, commit_batch_with_retry};
#[cfg(test)]
pub(crate) use dispatch_policy::notify_after_commit;
#[cfg(test)]
pub(crate) use dispatch_policy::{
    LaunchAuthority, PromptAuthorization, SettingsLoadDecision, classify_launch_result,
    classify_settings_load, prompt_authorization_after_binding,
};
use dispatch_policy::{claim_failure_backoff, classify_claim_failure};
use dispatch_support::{
    add_duration, at_or_after, claim_renew_interval, mint_dispatch_owner, mint_item_id,
    mint_patch_id, wait_for_next_claim, wall_clock,
};
pub(crate) use dispatch_support::{binding_bytes_vec, binding_matches_bytes};
#[cfg(test)]
pub(crate) use observation_commit::{
    SubagentCommitCursor, commit_activity_observation, commit_subagent_observation,
};
use recovery::{run_final_recovery_page, run_recovery_pages, shutdown_owner_bounded};
#[cfg(test)]
use steer::handle_steer;
#[cfg(test)]
use turn::{TurnConsumptionContext, TurnConsumptionState, consume_turn, handle_observation};

const PROMPT_DELIVERY_MAX_BYTES: usize = 256;
const PROVIDER_BINDING_VERSION: i64 = 1;
const PROVIDER_FAILURE_CODE: &str = "provider_failed";
const PROVIDER_FAILURE_MESSAGE: &str = "OpenCode2 provider turn failed";
const INTERRUPTED_CODE: &str = "provider_interrupted";
const INTERRUPTED_MESSAGE: &str = "OpenCode2 provider turn interrupted";

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
    /// The configured shutdown budget elapsed; the dispatcher stopped
    /// waiting for the join so shutdown itself stays bounded.
    BudgetExceeded,
    /// The dispatcher joined, but its engine owner quarantined and retained
    /// child custody beyond the bounded settle grace.
    Quarantined,
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
    interactions: RunInteractionRegistry,
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
    /// Only the test-only fixture launch mode can exhaust its capability, so
    /// production builds never construct this variant.
    #[allow(dead_code)]
    Exhausted,
}

enum ResolvedLaunch {
    Configured(Box<VerifiedOpenCode2ProfileLaunch>),
    Codex(Box<VerifiedCodexLaunch>),
    Claude(Box<VerifiedClaudeLaunch>),
    Cursor(Box<CursorLaunch>),
    Grok(Box<GrokLaunch>),
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
    #[cfg(test)]
    #[must_use]
    pub(crate) fn start(
        repository: Repository,
        database_path: PathBuf,
        config: NativeRunDispatcherConfig,
        process_cancel: Arc<CancelHandle>,
        activity: ActivityGateImpl,
        runtime: &Handle,
    ) -> Self {
        // `queue_capacity` is a `NonZeroUsize`; the registry's only
        // constructor failure is zero capacity.
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
    /// authenticated `StopRun` requests. The registry is created by Forge
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

    #[expect(
        clippy::too_many_arguments,
        reason = "fixture-only constructor threading the dispatcher's real dependencies; a one-use parameter struct adds no meaning"
    )]
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
        // `queue_capacity` is a `NonZeroUsize`; the registry's only
        // constructor failure is zero capacity.
        let interactions = RunInteractionRegistry::new(config.queue_capacity.get())
            .expect("dispatcher queue capacity should be nonzero");
        let join = runtime.spawn(dispatch_loop(DispatchLoopContext {
            repository,
            database_path,
            config,
            stop: Arc::clone(&stop),
            process_cancel,
            cancellation,
            interactions: interactions.clone(),
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
            interactions,
        }
    }

    /// Returns the process-owned live-run interaction registry shared with
    /// authenticated response routes.
    pub(crate) fn interaction_registry(&self) -> RunInteractionRegistry {
        self.interactions.clone()
    }

    pub(crate) fn catalog_client(&self) -> crate::engine_owner::EngineCatalogClient {
        self.catalog_client.clone()
    }

    /// Stops claims, drains admission, and awaits the owner within the
    /// configured budget. A budget breach or an owner quarantine is reported
    /// typed instead of awaiting the join without a bound.
    pub(crate) async fn shutdown(&mut self) -> NativeRunDispatcherShutdown {
        self.stop.cancel();
        if let Some(observed) = self.observed {
            return observed;
        }
        let Some(mut join) = self.join.take() else {
            self.observed = Some(NativeRunDispatcherShutdown::Joined);
            return NativeRunDispatcherShutdown::Joined;
        };
        let outcome = match tokio::time::timeout(self.shutdown_budget, &mut join).await {
            Ok(Ok(DispatchLoopExit { owner })) => match owner {
                EngineOwnerShutdown::Joined => NativeRunDispatcherShutdown::Joined,
                EngineOwnerShutdown::Quarantined => NativeRunDispatcherShutdown::Quarantined,
                EngineOwnerShutdown::TaskLost => NativeRunDispatcherShutdown::TaskLost,
            },
            Ok(Err(_)) => NativeRunDispatcherShutdown::TaskLost,
            Err(_) => NativeRunDispatcherShutdown::BudgetExceeded,
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
    interactions: RunInteractionRegistry,
    owner: EngineOwner,
    activity: ActivityGateImpl,
    launch_mode: DispatchLaunchMode,
}

#[expect(
    clippy::too_many_lines,
    reason = "claims arrive through one sequential state machine; splitting would scatter the stop/wait flow across helpers"
)]
async fn dispatch_loop(context: DispatchLoopContext) -> DispatchLoopExit {
    let DispatchLoopContext {
        repository,
        database_path,
        config,
        stop,
        process_cancel,
        cancellation,
        interactions,
        mut owner,
        activity,
        mut launch_mode,
    } = context;
    let origin = SystemCommandOrigin;
    // Fail steered rows orphaned by a previous process lifetime exactly
    // once here, while no turn loop can be pumping: rows whose target run
    // is still live cannot exist at this point (leases die with their
    // loops), so every open steered row left behind is terminally
    // unrecoverable in place. Payloads stay preserved for user recovery.
    // A failed sweep never blocks the loop; the next start retries it.
    if let Some(operated_at) = wall_clock(&origin) {
        let _ = repository
            .fail_orphaned_steered_dispatches(operated_at)
            .await;
    }
    // `AcceptedTurn::finish` can report an unresolved reap while the owner
    // quarantines a retained child. Keep its activity lease until owner
    // shutdown proves that custody has resolved.
    let mut retained_activity = Vec::new();
    let mut claim_failures = 0_u32;
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
        let claimed = match repository.claim_next_message_dispatch(claim).await {
            Ok(Some(claimed)) => {
                claim_failures = 0;
                claimed
            }
            Ok(None) => {
                claim_failures = 0;
                drop(activity_lease);
                if !wait_for_next_claim(&stop, &process_cancel, config.poll_interval).await {
                    break;
                }
                continue;
            }
            Err(error) => {
                drop(activity_lease);
                let disposition = classify_claim_failure(&error);
                let backoff =
                    claim_failure_backoff(disposition, claim_failures, config.retry_backoff);
                claim_failures = claim_failures.saturating_add(1);
                eprintln!("native run dispatch claim attempt failed ({disposition:?}): {error}");
                if !wait_for_next_claim(&stop, &process_cancel, backoff).await {
                    break;
                }
                continue;
            }
        };
        if let Some(lease) = Box::pin(execute_claim(
            ClaimExecution {
                repository: &repository,
                database_path: Path::new(&database_path),
                config: &config,
                origin: &origin,
                stop: &stop,
                process_cancel: &process_cancel,
                cancellation: &cancellation,
                interactions: &interactions,
                owner: &owner,
                claimed,
            },
            launch_mode,
            activity_lease,
        ))
        .await
        {
            retained_activity.push(lease);
        }
    }

    run_final_recovery_page(&repository, &config, &origin).await;

    let owner_shutdown = shutdown_owner_bounded(&mut owner).await;
    // The owner shutdown observation is bounded even while unresolved child
    // custody remains (typed `Quarantined`), so the retained activity leases
    // are released at this boundary; the parked owner task keeps the exact
    // custody and stays observable through the owner facade.
    drop(retained_activity);
    DispatchLoopExit {
        owner: owner_shutdown,
    }
}

/// Runs one provider turn while heartbeating its dispatch lease.
///
/// The turn may legitimately outlive the original claim window, and the
/// recovery sweep only ever runs between turns: without the heartbeat a turn
/// that settles after its old expiry would be fenced out of its own
/// settlement and reaped as an unknown outcome. A failed renewal is never
/// fatal by itself; if the lease truly lapsed the recovery sweep owns the
/// outcome exactly as before.
pub(crate) async fn drive_turn_with_lease_heartbeat<F>(
    repository: &Repository,
    origin: &SystemCommandOrigin,
    claimed: &ClaimedMessageDispatch,
    claim_lease: Duration,
    turn: F,
) -> F::Output
where
    F: std::future::Future,
{
    renew_claim_lease(repository, origin, claimed, claim_lease).await;
    tokio::pin!(turn);
    let renew_interval = claim_renew_interval(claim_lease);
    let mut renew_at =
        tokio::time::interval_at(tokio::time::Instant::now() + renew_interval, renew_interval);
    renew_at.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            output = &mut turn => break output,
            _ = renew_at.tick() => {
                renew_claim_lease(repository, origin, claimed, claim_lease).await;
            }
        }
    }
}

/// Best-effort renewal of the live dispatch lease.
async fn renew_claim_lease(
    repository: &Repository,
    origin: &SystemCommandOrigin,
    claimed: &ClaimedMessageDispatch,
    claim_lease: Duration,
) {
    let Some(operated_at) = wall_clock(origin) else {
        return;
    };
    let Some(lease_expires_at) = add_duration(operated_at, claim_lease) else {
        return;
    };
    let _ = repository
        .renew_message_dispatch_lease(
            &claimed.message_id,
            &claimed.owner,
            operated_at,
            lease_expires_at,
        )
        .await;
}

struct ClaimExecution<'a> {
    repository: &'a Repository,
    database_path: &'a Path,
    config: &'a NativeRunDispatcherConfig,
    origin: &'a SystemCommandOrigin,
    stop: &'a CancelHandle,
    process_cancel: &'a CancelHandle,
    cancellation: &'a RunCancellationRegistry,
    interactions: &'a RunInteractionRegistry,
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
    engine: EngineId,
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

#[cfg(test)]
#[path = "../../../tests/backend/steer_drive.rs"]
mod steer_drive_tests;

#[cfg(test)]
#[path = "../../../tests/backend/visible_stream_delivery.rs"]
mod visible_stream_delivery_tests;
