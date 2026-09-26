//! Provider startup under the claim-lease heartbeat and the launch deadline.
//!
//! The `slow_start_then_terminal` fixture stays silent for 1.5 s before it
//! announces readiness: five times the 300 ms claim lease used here. A live
//! recovery sweep runs throughout, standing in for the other dispatcher
//! workers that reap expired leases in production.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use artisan_database::entities::{self, AssistantRunLifecycle, DispatchState, EntityLifecycle};
use artisan_database::{QueueFirstMessageInput, Repository, StartupReconciliationCandidate};
use artisan_domain::{MessageBody, MessageId, PatchId, RequestId, ThreadId, UnixMillis};
use artisan_native_engine::NativeOpenCode2Authority;
use artisan_transport::CancelHandle;
use sea_orm::EntityTrait;

use super::{
    fetch_all, registered_fixture_program, seed_project_and_thread_with_profile, temp_repository,
};
use crate::conversation_commit_notifier::ConversationCommitNotifier;
use crate::lifecycle_control::ActivityGateImpl;
use crate::native_run_dispatch::{
    FixtureScenarioLaunch, NativeRunDispatcher, NativeRunDispatcherConfig,
    NativeRunDispatcherConfigInput, NativeRunDispatcherShutdown,
};
use crate::run_cancellation::RunCancellationRegistry;
use crate::startup_reconciliation_sweep::{
    PatchSourceError, StartupReconciliationPatchSource, StartupReconciliationPatches,
    StartupReconciliationSweepInput, sweep_startup_reconciliation,
};

const SLOW_START_SCENARIO: &str = "slow_start_then_terminal";
const CLAIM_LEASE: Duration = Duration::from_millis(300);
const SETTLE_DEADLINE: Duration = Duration::from_secs(15);

fn start_config(launch_deadline: Duration) -> NativeRunDispatcherConfig {
    NativeRunDispatcherConfig::new(
        NativeOpenCode2Authority::new(),
        ConversationCommitNotifier::new(),
        NativeRunDispatcherConfigInput {
            claim_lease: CLAIM_LEASE,
            launch_deadline,
            poll_interval: Duration::from_millis(10),
            retry_backoff: Duration::from_millis(10),
            shutdown_budget: Duration::from_secs(5),
            queue_capacity: std::num::NonZeroUsize::new(1).expect("one queue slot"),
            max_command_retries: std::num::NonZeroUsize::new(3).expect("three retries"),
            prompt_delivery: "immediate".to_owned(),
            stream_after: 0,
        },
    )
    .expect("start dispatch policy")
}

struct LivePatches(AtomicU64);

impl StartupReconciliationPatchSource for LivePatches {
    fn patch_ids_for(
        &mut self,
        candidate: &StartupReconciliationCandidate,
    ) -> Result<StartupReconciliationPatches, PatchSourceError> {
        let next = self.0.fetch_add(2, Ordering::Relaxed);
        let turn = PatchId::parse(format!("live-sweep-{next}")).map_err(|_| PatchSourceError)?;
        let item = candidate
            .assistant_item_id
            .as_ref()
            .map(|_| PatchId::parse(format!("live-sweep-{}", next + 1)))
            .transpose()
            .map_err(|_| PatchSourceError)?;
        Ok(StartupReconciliationPatches::new(turn, item))
    }
}

/// Sweeps expired leases the way another live worker would, until `stop`.
async fn live_sweeper(repository: Repository, stop: Arc<CancelHandle>) -> usize {
    let mut source = LivePatches(AtomicU64::new(0));
    let mut interrupted = 0;
    while !stop.is_cancelled() {
        let now = crate::CommandOrigin::acceptance_instant(&crate::SystemCommandOrigin)
            .expect("wall clock");
        let input = StartupReconciliationSweepInput::live_lease_expiry(now, 64).expect("input");
        if let Ok(report) = sweep_startup_reconciliation(&repository, input, &mut source).await {
            interrupted += report.interrupted;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    interrupted
}

struct StartRun {
    run: entities::AssistantRun,
    dispatch: entities::MessageDispatch,
    turn: entities::ConversationTurn,
    swept: usize,
}

async fn run_slow_start(label: &str, launch_deadline: Duration) -> StartRun {
    run_start(label, SLOW_START_SCENARIO, launch_deadline).await
}

async fn run_start(label: &str, scenario: &'static str, launch_deadline: Duration) -> StartRun {
    let (database, repository, temp) = temp_repository(label).await;
    let thread_id = ThreadId::parse("fixture-thread").expect("thread id");
    seed_project_and_thread_with_profile(
        &database,
        &repository,
        thread_id.as_str(),
        "fixture-test",
        30_000,
        5_000,
        2_000,
    )
    .await;
    repository
        .queue_first_message(QueueFirstMessageInput {
            request_id: RequestId::parse("fixture-request").expect("request id"),
            message_id: MessageId::parse("fixture-message").expect("message id"),
            thread_id,
            body: MessageBody::parse("hello world").expect("message body"),
            accepted_at: UnixMillis::from_millis(50),
        })
        .await
        .expect("fixture message should queue");

    let stop_sweeper = Arc::new(CancelHandle::new());
    let sweeper = tokio::spawn(live_sweeper(repository.clone(), Arc::clone(&stop_sweeper)));
    let process_cancel = Arc::new(CancelHandle::new());
    let mut dispatcher =
        NativeRunDispatcher::start_with_fixture_scenario_for_tests(FixtureScenarioLaunch {
            repository: repository.clone(),
            database_path: temp.path().to_owned(),
            config: start_config(launch_deadline),
            process_cancel: Arc::clone(&process_cancel),
            cancellation: RunCancellationRegistry::new(1).expect("registry capacity"),
            activity: ActivityGateImpl::new(),
            runtime: &tokio::runtime::Handle::current(),
            fixture_program: registered_fixture_program(),
            scenario,
        });
    tokio::time::timeout(SETTLE_DEADLINE, async {
        loop {
            let dispatch = entities::message_dispatch::Entity::find_by_id("fixture-message")
                .one(&database)
                .await
                .expect("dispatch query")
                .expect("dispatch exists");
            if matches!(
                dispatch.state,
                DispatchState::Completed | DispatchState::Failed
            ) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the slow-start dispatch should settle");
    stop_sweeper.cancel();
    let swept = sweeper.await.expect("sweeper joins");
    process_cancel.cancel();
    assert_eq!(
        dispatcher.shutdown().await,
        NativeRunDispatcherShutdown::Joined
    );
    let after = fetch_all(&database).await;
    StartRun {
        run: after.runs.into_iter().next().expect("one launched run"),
        dispatch: after.dispatches.into_iter().next().expect("one dispatch"),
        turn: after.turns.into_iter().next().expect("one turn"),
        swept,
    }
}

#[tokio::test(flavor = "current_thread")]
async fn dispatch_provider_start_slower_than_the_lease_completes() {
    let started = run_slow_start("start-slow-lease", Duration::from_secs(10)).await;
    assert_eq!(
        started.swept, 0,
        "live recovery must not reap an owned start"
    );
    assert_eq!(started.dispatch.state, DispatchState::Completed);
    assert!(started.dispatch.last_error.is_none());
    assert_eq!(started.run.lifecycle, AssistantRunLifecycle::Completed);
    assert!(started.run.error_code.is_none());
    assert!(started.run.provider_binding.is_some());
}

#[tokio::test(flavor = "current_thread")]
async fn dispatch_provider_start_past_its_deadline_fails_with_the_launch_error() {
    let started = run_slow_start("start-deadline", Duration::from_millis(400)).await;
    assert_eq!(
        started.swept, 0,
        "a known launch failure is not an unknown outcome"
    );
    assert_eq!(started.dispatch.state, DispatchState::Failed);
    assert_eq!(
        started.dispatch.last_error.as_deref(),
        Some("OpenCode did not start within 400 ms.")
    );
    assert_eq!(started.run.lifecycle, AssistantRunLifecycle::Failed);
    assert_eq!(
        started.run.error_code.as_deref(),
        Some("provider_start_timeout")
    );
    assert!(started.run.provider_binding.is_none());
    assert!(started.run.terminal_at_ms.is_some());
    assert_eq!(started.turn.lifecycle, EntityLifecycle::Failed);
}

/// Asserts a start that failed before announcing, returning its message.
fn assert_refused_start(started: &StartRun) -> String {
    assert_eq!(started.dispatch.state, DispatchState::Failed);
    assert_eq!(started.run.lifecycle, AssistantRunLifecycle::Failed);
    assert_eq!(
        started.run.error_code.as_deref(),
        Some("provider_start_failed")
    );
    assert!(started.run.provider_binding.is_none());
    assert_eq!(started.turn.lifecycle, EntityLifecycle::Failed);
    let message = started.dispatch.last_error.clone().expect("dispatch error");
    assert_eq!(started.run.error_message.as_deref(), Some(message.as_str()));
    message
}

#[tokio::test(flavor = "current_thread")]
async fn dispatch_provider_refusal_reports_the_engines_own_reason() {
    let started = run_start(
        "start-stderr-reason",
        "stderr_error_then_exit",
        Duration::from_secs(10),
    )
    .await;
    assert_eq!(
        assert_refused_start(&started),
        "OpenCode failed to start: something specific."
    );
}

#[tokio::test(flavor = "current_thread")]
async fn dispatch_provider_refusal_redacts_secrets_in_the_reason() {
    let started = run_start(
        "start-stderr-secret",
        "stderr_secret_then_exit",
        Duration::from_secs(10),
    )
    .await;
    let message = assert_refused_start(&started);
    assert_eq!(
        message,
        "OpenCode failed to start: login failed for ~/.config token=[redacted] key [redacted]."
    );
    for leaked in ["fixture-secret-value", "sk-fixture", "fixture-user"] {
        assert!(!message.contains(leaked), "{leaked} leaked into {message}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn dispatch_provider_stderr_never_surfaces_for_a_started_run() {
    let started = run_start(
        "start-stderr-noise",
        "stderr_noise_then_terminal",
        Duration::from_secs(10),
    )
    .await;
    assert_eq!(started.dispatch.state, DispatchState::Completed);
    assert!(started.dispatch.last_error.is_none());
    assert_eq!(started.run.lifecycle, AssistantRunLifecycle::Completed);
    assert!(started.run.error_code.is_none());
    assert!(started.run.error_message.is_none());
}
