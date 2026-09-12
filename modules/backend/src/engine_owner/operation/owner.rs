//! Owner task: the admission loop, generation minting, deadline and
//! cancellation rejection, and the quarantine tail.

use std::sync::Arc;

use artisan_transport::CancelHandle;
use tokio::sync::mpsc;
use tokio::sync::watch;
use tokio::time::Instant;

use super::super::EngineBounds;
use super::super::EngineLimits;
use super::super::process::LaunchRecipe;
use super::super::process::RetainedEngine;
use super::super::process::eventual_wait_once;
use super::bootstrap::execute_catalog_job;
use super::bootstrap::execute_preflight_job;
use super::core::EngineOperationError;
use super::core::Execution;
use super::core::GenerationAllocator;
use super::core::HealthState;
use super::core::Job;
use super::lifecycle::execute_configured_job;
use super::lifecycle::execute_legacy_job;

/// Runs the owner task until shutdown, channel closure, or quarantine.
pub(crate) async fn run_owner(
    jobs: mpsc::Receiver<Job>,
    shutdown: Arc<CancelHandle>,
    health: watch::Sender<HealthState>,
    recipe: LaunchRecipe,
    limits: EngineLimits,
    bounds: EngineBounds,
) {
    Box::pin(run_owner_with_allocator(
        jobs,
        shutdown,
        health,
        recipe,
        limits,
        bounds,
        GenerationAllocator::new(),
    ))
    .await;
}

/// Variant that starts from a caller-supplied allocator (test-seeded).
pub(crate) async fn run_owner_with_allocator(
    jobs: mpsc::Receiver<Job>,
    shutdown: Arc<CancelHandle>,
    health: watch::Sender<HealthState>,
    recipe: LaunchRecipe,
    limits: EngineLimits,
    bounds: EngineBounds,
    mut generations: GenerationAllocator,
) {
    Box::pin(run_owner_loop(
        jobs,
        shutdown,
        health,
        Some(LegacyOwnerConfig {
            recipe,
            limits,
            bounds,
        }),
        &mut generations,
    ))
    .await;
}

/// Runs the configured owner lane.  Unlike the legacy test lane it has no
/// executable, version, budget, or bound values of its own: each turn carries
/// the immutable persisted snapshot that must govern its attempt.
pub(crate) async fn run_configured_owner(
    jobs: mpsc::Receiver<Job>,
    shutdown: Arc<CancelHandle>,
    health: watch::Sender<HealthState>,
) {
    let mut generations = GenerationAllocator::new();
    Box::pin(run_owner_loop(
        jobs,
        shutdown,
        health,
        None,
        &mut generations,
    ))
    .await;
}

struct LegacyOwnerConfig {
    recipe: LaunchRecipe,
    limits: EngineLimits,
    bounds: EngineBounds,
}

async fn run_owner_loop(
    mut jobs: mpsc::Receiver<Job>,
    shutdown: Arc<CancelHandle>,
    health: watch::Sender<HealthState>,
    legacy: Option<LegacyOwnerConfig>,
    generations: &mut GenerationAllocator,
) {
    loop {
        tokio::select! {
            biased;

            () = shutdown.wait() => break,

            job = jobs.recv() => {
                let Some(job) = job else { break };
                if shutdown.is_cancelled() {
                    reject_job(job, EngineOperationError::Shutdown);
                    continue;
                }
                if job_control(&job).is_cancelled() {
                    reject_job(job, EngineOperationError::Cancelled);
                    continue;
                }
                if Instant::now() >= job_deadline(&job) {
                    reject_job(job, EngineOperationError::Deadline);
                    continue;
                }
                let Some(generation) = generations.mint() else {
                    let _ = health.send(HealthState::Quarantined);
                    reject_job(job, EngineOperationError::GenerationExhausted);
                    quarantine_tail(&mut jobs, None).await;
                    return;
                };

                let execution = match job {
                    job @ Job::Legacy { .. } => {
                        let Some(config) = legacy.as_ref() else {
                            reject_job(job, EngineOperationError::Configuration);
                            continue;
                        };
                        execute_legacy_job(
                            &config.recipe,
                            generation,
                            job,
                            &shutdown,
                            config.limits,
                            config.bounds,
                        )
                        .await
                    }
                    job @ Job::Preflight { .. } => {
                        Box::pin(execute_preflight_job(job, &shutdown)).await
                    }
                    job @ Job::Catalog { .. } => {
                        Box::pin(execute_catalog_job(job, &shutdown)).await
                    }
                    job @ Job::Turn { .. } => {
                        Box::pin(execute_configured_job(job, &shutdown)).await
                    }
                };
                match execution {
                    Execution::Completed => {}
                    Execution::Quarantined(retained) => {
                        let _ = health.send(HealthState::Quarantined);
                        quarantine_tail(&mut jobs, Some(retained)).await;
                        return;
                    }
                }
            }
        }
    }

    while let Ok(job) = jobs.try_recv() {
        reject_job(job, EngineOperationError::Shutdown);
    }
}

fn job_control(job: &Job) -> &Arc<CancelHandle> {
    match job {
        Job::Legacy { control, .. }
        | Job::Preflight { control, .. }
        | Job::Catalog { control, .. }
        | Job::Turn { control, .. } => control,
    }
}

fn job_deadline(job: &Job) -> Instant {
    match job {
        Job::Legacy { deadline, .. } | Job::Turn { deadline, .. } => *deadline,
        Job::Preflight { input, .. } => input.deadlines.admission,
        Job::Catalog { input, .. } => input.deadlines.admission.min(input.catalog_deadline),
    }
}

fn reject_job(job: Job, error: EngineOperationError) {
    match job {
        Job::Legacy { respond, .. } => {
            let _ = respond.send(Err(error));
        }
        Job::Preflight { respond, .. } => {
            let _ = respond.send(Err(error));
        }
        Job::Catalog { respond, .. } => {
            let _ = respond.send(Err(error));
        }
        Job::Turn {
            prepared, respond, ..
        } => {
            let _ = prepared.send(Err(error.clone()));
            let _ = respond.send(Err(error));
        }
    }
}

/// Serves the quarantine tail: closes admission immediately, drains the
/// already-bounded queue, and then resolves custody of the retained engine,
/// if any, exactly once.
async fn quarantine_tail(jobs: &mut mpsc::Receiver<Job>, retained: Option<Box<RetainedEngine>>) {
    jobs.close();
    while let Some(job) = jobs.recv().await {
        reject_job(job, EngineOperationError::Shutdown);
    }

    if let Some(engine) = retained {
        match eventual_wait_once(engine).await {
            Ok(_status) => {}
            Err(retained) => {
                let _custody = retained;
                std::future::pending::<()>().await;
                unreachable!("pending never resolves");
            }
        }
    }
}
