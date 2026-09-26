use std::{
    env,
    ffi::OsString,
    fmt,
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant},
};

use crate::{
    CliError, Result,
    error::{ForgeTermination, io},
};

use super::{
    FORGE_READY_INTERVAL, FORGE_SHUTDOWN_GRACE_MAX, FORGE_START_TIMEOUT,
    OWNED_FORGE_ALREADY_RUNNING, OWNED_FORGE_READINESS_FAILURE, OWNED_FORGE_SHUTDOWN_FAILURE,
    OWNED_FORGE_START_FAILURE,
    receipt::{
        BackgroundStartDecision, ReadinessFileRead, ReadinessFileSnapshot, ReadinessReconcile,
        background_start_decision, detach, process_executable, read_readiness_file,
        readiness_file_replaced, readiness_matches_child, readiness_status,
        reconcile_stale_readiness, sleep_until, wait_for_readiness_with,
    },
    spec::{
        ForgeLaunchSpec, ForgeReadiness, ForgeReadinessStatus, StartResult,
        ensure_forge_executable, forge_command, is_forbidden_environment_key,
    },
};
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum OwnedStartDecision {
    RefuseAlreadyRunning,
    Spawn {
        prior_readiness: Option<ReadinessFileSnapshot>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum OwnedProcessStartInfo {
    QueryFailed,
    Missing,
    MissingStartTime,
    Present(u64),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum OwnedProcessIdentity {
    Match,
    FailClosed,
}

pub(super) fn owned_start_decision<F>(
    existing: ReadinessFileRead,
    expected_executable: &Path,
    resolve_executable: F,
) -> OwnedStartDecision
where
    F: FnOnce(u32) -> Option<PathBuf>,
{
    match background_start_decision(existing, expected_executable, resolve_executable) {
        BackgroundStartDecision::AlreadyRunning => OwnedStartDecision::RefuseAlreadyRunning,
        BackgroundStartDecision::Spawn { prior_readiness } => {
            OwnedStartDecision::Spawn { prior_readiness }
        }
    }
}

fn forge_owned_command(spec: &ForgeLaunchSpec) -> processkit::Command {
    forge_owned_command_with_environment(spec, env::vars_os())
}

pub(super) fn forge_owned_command_with_environment<I>(
    spec: &ForgeLaunchSpec,
    variables: I,
) -> processkit::Command
where
    I: IntoIterator<Item = (OsString, OsString)>,
{
    let mut command = processkit::Command::new(spec.executable())
        .args(spec.argv())
        .env_clear()
        .stdin(processkit::Stdin::empty())
        .stdout(processkit::StdioMode::Null)
        .stderr(processkit::StdioMode::Null)
        .create_no_window();
    for (key, value) in variables {
        if !is_forbidden_environment_key(&key) {
            command = command.env(key, value);
        }
    }
    command
}

fn owned_process_start_info(pid: u32) -> OwnedProcessStartInfo {
    match processkit::process_info(pid) {
        Ok(Some(info)) => match info.start_time() {
            Some(start_time) => OwnedProcessStartInfo::Present(start_time),
            None => OwnedProcessStartInfo::MissingStartTime,
        },
        Ok(None) => OwnedProcessStartInfo::Missing,
        Err(_) => OwnedProcessStartInfo::QueryFailed,
    }
}

pub(super) fn owned_process_identity(
    expected_start_time: u64,
    current: OwnedProcessStartInfo,
) -> OwnedProcessIdentity {
    match current {
        OwnedProcessStartInfo::Present(current_start_time)
            if current_start_time == expected_start_time =>
        {
            OwnedProcessIdentity::Match
        }
        OwnedProcessStartInfo::QueryFailed
        | OwnedProcessStartInfo::Missing
        | OwnedProcessStartInfo::MissingStartTime
        | OwnedProcessStartInfo::Present(_) => OwnedProcessIdentity::FailClosed,
    }
}

pub(super) fn owned_process_identity_matches(
    expected_start_time: u64,
    current: OwnedProcessStartInfo,
) -> bool {
    owned_process_identity(expected_start_time, current) == OwnedProcessIdentity::Match
}

pub(super) fn owned_readiness_candidate<F>(
    current: ReadinessFileRead,
    prior_readiness: Option<&ReadinessFileSnapshot>,
    child_pid: u32,
    expected_executable: &Path,
    expected_start_time: u64,
    current_process_info: OwnedProcessStartInfo,
    resolve_executable: F,
) -> Option<ForgeReadiness>
where
    F: FnOnce(u32) -> Option<PathBuf>,
{
    if !owned_process_identity_matches(expected_start_time, current_process_info) {
        return None;
    }
    let ReadinessFileRead::Present(snapshot) = current else {
        return None;
    };
    if !readiness_file_replaced(prior_readiness, &snapshot) {
        return None;
    }
    let readiness = ForgeReadiness::from_json(&snapshot.bytes).ok()?;
    readiness_matches_child(
        &readiness,
        child_pid,
        expected_executable,
        resolve_executable,
    )
    .then_some(readiness)
}

fn owned_process_failure(context: &'static str) -> CliError {
    // Do not format a processkit error here: its diagnostic may include the
    // command's program or other launch details, while Forge argv contains
    // credential paths. The CLI boundary intentionally exposes only a bounded
    // lifecycle message.
    CliError::Control(context.to_owned())
}

pub(super) fn owned_already_running_failure() -> CliError {
    CliError::Unsupported(OWNED_FORGE_ALREADY_RUNNING.to_owned())
}

pub(super) fn owned_shutdown_failure() -> CliError {
    CliError::Io {
        context: "shutdown Forge",
        source: std::io::Error::other(OWNED_FORGE_SHUTDOWN_FAILURE),
    }
}

pub(super) fn forge_startup_outcome_error(outcome: processkit::Outcome) -> CliError {
    match outcome {
        processkit::Outcome::Exited(code) => CliError::ForgeTerminated {
            termination: ForgeTermination::from_code(Some(code)),
        },
        processkit::Outcome::Signalled(_) => CliError::ForgeTerminated {
            termination: ForgeTermination::from_code(None),
        },
        _ => owned_process_failure(OWNED_FORGE_START_FAILURE),
    }
}

pub(super) fn clamp_shutdown_grace(requested_grace: Duration) -> Duration {
    requested_grace.min(FORGE_SHUTDOWN_GRACE_MAX)
}

async fn owned_child_has_exited(process: &mut processkit::RunningProcess) -> Result<bool> {
    // `wait_for` is the public processkit nonblocking exit-observation seam:
    // the false predicate never accepts readiness, while the zero-duration
    // probe still reaps and caches an already-exited child for `wait()` below.
    match process.wait_for(|| async { false }, Duration::ZERO).await {
        Ok(()) => Ok(false),
        Err(error) => match error.into_reason() {
            processkit::ErrorReason::NotReady { .. } => Ok(process.pid().is_none()),
            _ => Err(owned_process_failure(OWNED_FORGE_READINESS_FAILURE)),
        },
    }
}

async fn owned_child_exit_failure<T>(process: processkit::RunningProcess) -> Result<T> {
    match process.wait().await {
        Ok(outcome) => Err(forge_startup_outcome_error(outcome)),
        Err(_) => Err(owned_process_failure(OWNED_FORGE_START_FAILURE)),
    }
}

async fn teardown_owned_process<T>(
    process: processkit::RunningProcess,
    failure: CliError,
) -> Result<T> {
    match process.shutdown(Duration::ZERO).await {
        Ok(_) => Err(failure),
        Err(_) => Err(owned_shutdown_failure()),
    }
}

async fn owned_failure_after_probe<T>(
    process: processkit::RunningProcess,
    failure: CliError,
) -> Result<T> {
    let mut process = process;
    match owned_child_has_exited(&mut process).await {
        Ok(true) => owned_child_exit_failure(process).await,
        Ok(false) => teardown_owned_process(process, failure).await,
        Err(error) => teardown_owned_process(process, error).await,
    }
}

/// A live Forge process whose process tree is owned by the caller.
///
/// The normal lifecycle ordering is for the owner to stop accepting new work,
/// send the Forge's normal stop/control commands, and await the transport
/// session's shutdown before calling [`Self::shutdown`]. This CLI layer
/// deliberately does not import transport; it only performs the final bounded
/// process-custody step. Dropping a live lease is an emergency hard-kill
/// backstop, not a substitute for that orderly shutdown.
pub struct ForgeProcessLease {
    pub(super) process: processkit::RunningProcess,
    pub(super) pid: u32,
    pub(super) readiness: ForgeReadiness,
}

impl fmt::Debug for ForgeProcessLease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ForgeProcessLease")
            .field("pid", &self.pid)
            .finish_non_exhaustive()
    }
}

impl ForgeProcessLease {
    pub fn readiness(&self) -> &ForgeReadiness {
        &self.readiness
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Finish releasing the owned Forge process tree with a bounded grace.
    ///
    /// The caller must have already sent normal Forge stop/control commands and
    /// awaited its transport session shutdown. `processkit` applies its
    /// signal/grace/escalation policy to the owned group on Unix, after the
    /// requested grace is clamped to [`FORGE_SHUTDOWN_GRACE_MAX`]. On Windows,
    /// the crate documents that graceful signal delivery degrades to an atomic
    /// Job Object kill, so this method does not claim a Windows grace period.
    /// The consuming processkit operation confirms normal exit or tree
    /// termination; a failure is returned as a bounded [`CliError::Io`].
    pub async fn shutdown(self, requested_grace: Duration) -> Result<()> {
        self.process
            .shutdown(clamp_shutdown_grace(requested_grace))
            .await
            .map(|_| ())
            .map_err(|_| owned_shutdown_failure())
    }
}

/// Start a newly owned Forge and wait for its replacement readiness receipt.
///
/// An existing live receipt is intentionally refused: this API never adopts a
/// Forge launched through the detached CLI path. The returned lease owns the
/// processkit private group, including the Windows Job Object containment path.
pub async fn start_owned(spec: &ForgeLaunchSpec) -> Result<ForgeProcessLease> {
    start_owned_until(spec, Instant::now() + FORGE_START_TIMEOUT).await
}

pub async fn start_owned_until(
    spec: &ForgeLaunchSpec,
    readiness_deadline: Instant,
) -> Result<ForgeProcessLease> {
    ensure_forge_executable(spec)?;
    let prior_readiness = match owned_start_decision(
        read_readiness_file(spec.readiness_path()),
        spec.executable(),
        process_executable,
    ) {
        OwnedStartDecision::RefuseAlreadyRunning => {
            return Err(owned_already_running_failure());
        }
        OwnedStartDecision::Spawn { prior_readiness } => prior_readiness,
    };

    let mut process = forge_owned_command(spec)
        .start()
        .await
        .map_err(|_| owned_process_failure(OWNED_FORGE_START_FAILURE))?;
    let Some(pid) = process.pid() else {
        return owned_failure_after_probe(
            process,
            owned_process_failure(OWNED_FORGE_START_FAILURE),
        )
        .await;
    };
    if !process.kills_tree_on_drop() {
        return owned_failure_after_probe(
            process,
            owned_process_failure(OWNED_FORGE_START_FAILURE),
        )
        .await;
    }

    let launch_start_time = match owned_process_start_info(pid) {
        OwnedProcessStartInfo::Present(start_time) => start_time,
        OwnedProcessStartInfo::QueryFailed
        | OwnedProcessStartInfo::Missing
        | OwnedProcessStartInfo::MissingStartTime => {
            return owned_failure_after_probe(
                process,
                owned_process_failure(OWNED_FORGE_START_FAILURE),
            )
            .await;
        }
    };

    let readiness_path = spec.readiness_path().to_path_buf();
    let expected_executable = spec.executable().to_path_buf();
    let prior_readiness_ref = prior_readiness.as_ref();

    loop {
        match owned_child_has_exited(&mut process).await {
            Ok(true) => return owned_child_exit_failure(process).await,
            Ok(false) => {}
            Err(error) => return teardown_owned_process(process, error).await,
        }

        if Instant::now() >= readiness_deadline {
            return owned_failure_after_probe(process, CliError::ForgeReadinessTimeout).await;
        }

        let current_process_info = owned_process_start_info(pid);
        if !owned_process_identity_matches(launch_start_time, current_process_info) {
            return owned_failure_after_probe(
                process,
                owned_process_failure(OWNED_FORGE_READINESS_FAILURE),
            )
            .await;
        }

        if let Some(readiness) = owned_readiness_candidate(
            read_readiness_file(&readiness_path),
            prior_readiness_ref,
            pid,
            &expected_executable,
            launch_start_time,
            current_process_info,
            process_executable,
        ) {
            if Instant::now() >= readiness_deadline {
                return owned_failure_after_probe(process, CliError::ForgeReadinessTimeout).await;
            }

            let final_process_info = owned_process_start_info(pid);
            if !owned_process_identity_matches(launch_start_time, final_process_info) {
                return owned_failure_after_probe(
                    process,
                    owned_process_failure(OWNED_FORGE_READINESS_FAILURE),
                )
                .await;
            }
            if Instant::now() >= readiness_deadline {
                return owned_failure_after_probe(process, CliError::ForgeReadinessTimeout).await;
            }

            return Ok(ForgeProcessLease {
                process,
                pid,
                readiness,
            });
        }

        let remaining = readiness_deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return owned_failure_after_probe(process, CliError::ForgeReadinessTimeout).await;
        }
        tokio::time::sleep(remaining.min(FORGE_READY_INTERVAL)).await;
    }
}

pub fn start_until(
    spec: &ForgeLaunchSpec,
    foreground: bool,
    readiness_deadline: Instant,
) -> Result<StartResult> {
    if foreground {
        supervise(spec, readiness_deadline, &mut |_| Ok(()))
    } else {
        spawn_background_forge(spec, readiness_deadline)
    }
}

/// Runs the Forge in the foreground until it exits, as a service manager's
/// main process does.
///
/// A readiness receipt left by a Forge that was killed is reconciled first
/// (see [`super::reconcile_stale_readiness`]), so a restart after a crash
/// is never blocked by it. Once the Forge publishes readiness, `on_ready`
/// runs (for example to publish a host invitation); a failure there stops
/// the Forge and is returned, so a supervisor restarts it.
///
/// # Errors
///
/// Returns [`CliError`] when a live Forge already runs from this home, the
/// Forge fails before readiness, `on_ready` fails, or the Forge exits
/// unsuccessfully.
pub fn supervise(
    spec: &ForgeLaunchSpec,
    readiness_deadline: Instant,
    on_ready: &mut dyn FnMut(&ForgeReadiness) -> Result<()>,
) -> Result<StartResult> {
    ensure_forge_executable(spec)?;
    if let ReadinessReconcile::CleanedStale { pid } = reconcile_stale_readiness(
        spec.readiness_path(),
        spec.custody_path(),
        spec.executable(),
    )? {
        eprintln!("removed the readiness receipt of exited Forge pid {pid}");
    }
    let mut child = forge_command(spec)
        .stdin(Stdio::null())
        .spawn()
        .map_err(io("start Forge"))?;
    let ready = wait_for_readiness_with(
        &mut child,
        spec.executable(),
        spec.readiness_path(),
        None,
        readiness_deadline,
        read_readiness_file,
        process_executable,
        sleep_until,
    )
    .and_then(
        |_| match readiness_status(spec.readiness_path(), spec.executable()) {
            ForgeReadinessStatus::Ready(readiness) => on_ready(&readiness),
            ForgeReadinessStatus::Missing | ForgeReadinessStatus::Invalid => {
                Err(CliError::ForgeReadinessTimeout)
            }
        },
    );
    if let Err(error) = ready {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    let status = child.wait().map_err(io("wait for Forge"))?;
    if status.success() {
        Ok(StartResult::ForegroundExited)
    } else {
        Err(CliError::ForgeTerminated {
            termination: ForgeTermination::from_exit_status(&status),
        })
    }
}

fn spawn_background_forge(
    spec: &ForgeLaunchSpec,
    readiness_deadline: Instant,
) -> Result<StartResult> {
    ensure_forge_executable(spec)?;
    let prior_readiness = match background_start_decision(
        read_readiness_file(spec.readiness_path()),
        spec.executable(),
        process_executable,
    ) {
        BackgroundStartDecision::AlreadyRunning => return Ok(StartResult::AlreadyRunning),
        BackgroundStartDecision::Spawn { prior_readiness } => prior_readiness,
    };
    let mut command = forge_command(spec);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    detach(&mut command);
    let mut child = command.spawn().map_err(io("start Forge"))?;
    wait_for_readiness_with(
        &mut child,
        spec.executable(),
        spec.readiness_path(),
        prior_readiness.as_ref(),
        readiness_deadline,
        read_readiness_file,
        process_executable,
        sleep_until,
    )
}
