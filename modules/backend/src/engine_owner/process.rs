//! Private process and stream ownership for the engine owner.
//!
//! Everything here serves the single owner task in
//! [`crate::engine_owner::operation`]: exact-child spawning, the sole
//! stdin lifeline writer, count-only stderr draining, and the fixed cleanup
//! sequence that ends in an observed reap or retained custody. Spawning uses
//! the caller-supplied absolute engine executable with an explicit empty
//! environment (`env_clear`), documenting the refusal to inherit ambient
//! state; production argv and environment are a separately frozen later
//! contract and no public API can reach a production spawn in this packet.
//! P3 adds explicit child environment `OPENCODE_PASSWORD=<secret>` and
//! `OPENCODE_SERVER_PASSWORD=` derived from a fresh 32-byte OS secret that
//! is never logged or cloned. No detached per-pipe reader tasks exist; the
//! owner interleaves waits and control signals on one task.

#[cfg(test)]
use std::cell::Cell;
#[cfg(test)]
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

#[cfg(test)]
use base64::Engine as _;
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout};

use artisan_domain::RootPath;
use artisan_native_engine::VerifiedOpenCode2ProfileLaunch;

/// The exact executable launch the owner task performs.
///
/// Production always launches the caller-supplied absolute engine executable
/// with an explicitly cleared environment. The `cfg(test)` variant exists
/// solely for the NEXT runtime-gated packet; it is declared here for shape
/// completeness and is never constructed outside tests by this packet.
pub(crate) enum LaunchRecipe {
    /// The trusted production engine executable path supplied by the caller.
    Production {
        /// Exact absolute executable path; never replaced or discovered.
        executable: PathBuf,
    },
    /// Test-only recipe for future runtime-gated coverage.
    #[cfg(test)]
    Fixture {
        /// The declared fixture executable.
        program: PathBuf,
        /// Explicit fixture arguments.
        args: Vec<OsString>,
        /// Child-side scenario name.
        scenario: &'static str,
    },
}

/// Spawns one engine child from the supplied recipe with the P3 secret.
///
/// All three standard streams are piped, `kill_on_drop(true)` is set so the
/// final `Child` drop is best-effort containment, and on Windows
/// `CREATE_NO_WINDOW` is applied. For `Production`, `env_clear()` is called
/// with no arguments: the explicit refusal of ambient inheritance, not a
/// shipping launch claim. The child environment then receives exactly
/// `OPENCODE_PASSWORD=<secret>` and `OPENCODE_SERVER_PASSWORD=` and no
/// ambient auth is read. The fixture variant also receives the same two
/// variables plus a synthetic `ARTISAN_ENGINE_OWNER_TEST_AUTHORIZATION`
/// `Basic base64(opencode:<secret>)` so the isolated fixture can validate
/// the single health request.
///
/// # Errors
///
/// Returns the raw spawn failure; the caller reduces it to a typed,
/// payload-free cause and never surfaces the operating-system message.
pub(crate) fn spawn_engine(recipe: &LaunchRecipe, secret: &str) -> io::Result<Child> {
    let mut command = match recipe {
        LaunchRecipe::Production { executable } => {
            let mut command = tokio::process::Command::new(executable);
            command.env_clear();
            command.env("OPENCODE_PASSWORD", secret);
            command.env("OPENCODE_SERVER_PASSWORD", "");
            command
        }
        #[cfg(test)]
        LaunchRecipe::Fixture {
            program,
            args,
            scenario,
        } => {
            let mut command = tokio::process::Command::new(program);
            command.env("ARTISAN_ENGINE_OWNER_TEST_SCENARIO", scenario);
            command.env("OPENCODE_PASSWORD", secret);
            command.env("OPENCODE_SERVER_PASSWORD", "");
            // Synthetic fixture auth so the fixture's health validator sees the
            // exact Basic value derived from the same secret.
            let credentials = format!("opencode:{secret}");
            let encoded = base64::engine::general_purpose::STANDARD.encode(credentials.as_bytes());
            let auth_value = format!("Basic {encoded}");
            command.env("ARTISAN_ENGINE_OWNER_TEST_AUTHORIZATION", auth_value);
            for argument in args {
                command.arg(argument);
            }
            command
        }
    };

    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command.kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let child = command.spawn()?;
    witness_spawned();
    Ok(child)
}

/// Test-only launch seam for the configured fixture.
///
/// This spawns the existing `prompt_text_then_terminal` fixture binary as the
/// configured child, using the same secret-derived `Basic` credential as the
/// production path via `HealthSecret::basic_auth`. It is `#[cfg(test)]` only
/// and never reachable in a normal production build.
#[cfg(test)]
pub(crate) fn spawn_configured_fixture_engine(
    program: &Path,
    scenario: &'static str,
    secret: &str,
) -> io::Result<Child> {
    let mut command = tokio::process::Command::new(program);
    command
        .env_clear()
        .env("ARTISAN_ENGINE_OWNER_TEST_SCENARIO", scenario)
        .env("OPENCODE_PASSWORD", secret)
        .env("OPENCODE_SERVER_PASSWORD", "");
    let secret_obj = crate::engine_owner::http::HealthSecret::from_raw_for_tests(secret.to_owned());
    let auth_value = secret_obj.basic_auth();
    command.env("ARTISAN_ENGINE_OWNER_TEST_AUTHORIZATION", auth_value);
    if let Ok(value) = std::env::var("SYSTEMROOT") {
        command.env("SYSTEMROOT", value);
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let child = command.spawn()?;
    witness_spawned();
    Ok(child)
}

/// Spawns the exact certified `OpenCode2` profile selected for one durable run.
///
/// Revalidation is deliberately the last authority operation before the
/// child is created.  The capability owns the installation fence for the
/// entire call, so a replacement generation cannot race the command.  The
/// environment is cleared and rebuilt only from the private profile roots,
/// explicit project root, and per-process credential supplied by the owner.
pub(crate) fn spawn_configured_engine(
    launch: &VerifiedOpenCode2ProfileLaunch,
    project_root: &RootPath,
    secret: &str,
) -> io::Result<Child> {
    let profile_home = launch.profile_home();
    let config = profile_home.join("config");
    let cache = profile_home.join("cache");
    let data = profile_home.join("data");
    let state = profile_home.join("state");
    let temp = profile_home.join("tmp");

    let mut command = tokio::process::Command::new(launch.executable_path());
    command
        .current_dir(Path::new(project_root.as_str()))
        .args(["serve", "--stdio", "--port", "0"])
        .env_clear()
        .env("OPENCODE_CONFIG_DIR", &config)
        .env("OPENCODE_DB", data.join("opencode.db"))
        .env("XDG_CACHE_HOME", &cache)
        .env("XDG_CONFIG_HOME", &config)
        .env("XDG_DATA_HOME", &data)
        .env("XDG_STATE_HOME", &state)
        .env("TMP", &temp)
        .env("TEMP", &temp)
        .env("OPENCODE_PASSWORD", secret)
        .env("OPENCODE_SERVER_PASSWORD", "")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    launch
        .revalidate()
        .map_err(|_| io::Error::new(io::ErrorKind::PermissionDenied, "profile rejected"))?;
    let child = command.spawn()?;
    witness_spawned();
    Ok(child)
}

/// The taken sole stdin writer kept open for the whole operation.
///
/// The writer is removed from the [`Child`] immediately after spawn so the
/// child's own `wait()` can never close it implicitly; it closes exactly
/// when the owner drops it.
pub(crate) struct LifelineWriter(Option<ChildStdin>);

impl LifelineWriter {
    /// Takes the child's piped stdin as the sole lifeline writer.
    pub(crate) fn take(child: &mut Child) -> Self {
        Self(child.stdin.take())
    }

    /// Closes the sole writer end explicitly.
    ///
    /// Every abnormal teardown calls this before waiting or killing.
    pub(crate) fn close(&mut self) {
        self.0 = None;
    }
}

/// Count-only stderr draining state.
///
/// Bytes are counted toward the caller-supplied `stderr_cap_bytes` and
/// discarded; nothing is ever retained, printed, or formatted. Terminal
/// states stop further reads but never discard the pipe handle.
pub(crate) struct StderrCounter {
    stderr: Option<ChildStderr>,
    counted: usize,
    cap: usize,
    state: StderrState,
}

/// Current counting state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StderrState {
    /// The stream is open and within the cap.
    Open,
    /// The cap was crossed; reads stopped while the handle stays retained.
    Capped,
    /// Clean end of stream within the cap.
    ClosedWithinCap,
    /// The operating-system read failed.
    Failed,
}

impl StderrCounter {
    /// Wraps the child's piped stderr with the caller-supplied cap.
    pub(crate) fn new(stderr: Option<ChildStderr>, cap: usize) -> Self {
        Self {
            stderr,
            counted: 0,
            cap,
            state: StderrState::Open,
        }
    }

    /// Current counting state.
    #[must_use]
    pub(crate) fn state(&self) -> StderrState {
        self.state
    }

    /// Performs at most one bounded counting read.
    ///
    /// Bytes beyond the cap are counted and discarded; content is never
    /// retained. Returns whether the cap was crossed or the stream closed.
    pub(crate) async fn pump(&mut self) -> StderrEvent {
        use tokio::io::AsyncReadExt as _;
        match self.state {
            StderrState::Capped => return StderrEvent::CapExceeded,
            StderrState::ClosedWithinCap => return StderrEvent::Closed,
            StderrState::Failed => return StderrEvent::ReadFailed,
            StderrState::Open => {}
        }
        let Some(stderr) = self.stderr.as_mut() else {
            self.state = StderrState::Failed;
            return StderrEvent::ReadFailed;
        };
        let mut buf = [0_u8; 512];
        let window = self.cap.saturating_sub(self.counted).saturating_add(1);
        let window = window.min(buf.len());
        match stderr.read(&mut buf[..window]).await {
            Ok(0) => {
                self.state = StderrState::ClosedWithinCap;
                StderrEvent::Closed
            }
            Ok(count) => {
                self.counted += count;
                if self.counted > self.cap {
                    self.state = StderrState::Capped;
                    StderrEvent::CapExceeded
                } else {
                    StderrEvent::WithinCap
                }
            }
            Err(_) => {
                self.state = StderrState::Failed;
                StderrEvent::ReadFailed
            }
        }
    }
}

/// What one stderr pump step observed.
pub(crate) enum StderrEvent {
    /// More bytes were counted; the stream stays open within the cap.
    WithinCap,
    /// The cap was crossed; counting stopped permanently.
    CapExceeded,
    /// Clean end of stream within the cap.
    Closed,
    /// The operating-system read failed.
    ReadFailed,
}

/// Exact custody retained when a child's death could not be observed.
pub(crate) struct RetainedEngine {
    /// The exact child, boxed for long-term ownership.
    pub(crate) child: Box<Child>,
    /// The closed sole writer, kept as part of exact custody.
    pub(crate) lifeline: LifelineWriter,
    /// The stderr counting state at abandonment.
    pub(crate) stderr_counter: StderrCounter,
}

/// How an abnormal sequence ended.
pub(crate) enum CleanupObservation {
    /// The child exited and was reaped without needing a kill.
    ReapedWithoutKill(ExitStatus),
    /// Termination was requested and the exit was then actually observed.
    ReapedAfterKill(ExitStatus),
    /// No reap could be observed; exact custody is returned for quarantine.
    Retained(Box<RetainedEngine>),
}

/// The complete set of resources one active operation owns.
pub(crate) struct ChildParts {
    /// The exact child; kept owned until an observed reap or quarantine.
    pub(crate) child: Child,
    /// The sole stdin writer.
    pub(crate) lifeline: LifelineWriter,
    /// Owned stdout pipe for exactly one bounded readiness record.
    pub(crate) stdout: Option<ChildStdout>,
    /// Count-only stderr state.
    pub(crate) stderr_counter: StderrCounter,
}

/// Runs the fixed, uncancellable cleanup sequence for one unresolved child.
///
/// Order is contractual: close the sole lifeline writer first, then wait up
/// to the caller-supplied `close_budget`, request termination with
/// `start_kill` if no exit was observed, then wait for the remaining budget
/// (or once without waiting when the budget is already expired). Pipe
/// resources are released only together with a successful observed reap or
/// moved whole into [`RetainedEngine`].
pub(crate) async fn cleanup_after_abort(
    parts: ChildParts,
    close_budget: Duration,
) -> CleanupObservation {
    let ChildParts {
        mut child,
        mut lifeline,
        stdout: _stdout,
        stderr_counter,
    } = parts;

    lifeline.close();
    let start = tokio::time::Instant::now();
    let deadline = start.checked_add(close_budget);

    // First wait: bounded by close_budget (or immediate poll when ZERO).
    let first_wait = match deadline {
        Some(deadline) => wait_until(&mut child, deadline).await,
        None => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "close budget unrepresentable",
        )),
    };
    if let Ok(status) = first_wait {
        witness_reaped(status);
        drop(stderr_counter);
        drop(lifeline);
        return CleanupObservation::ReapedWithoutKill(status);
    }

    witness_kill_requested();
    let _termination_error = child.start_kill();
    // Second wait: remaining budget, or one immediate poll when ZERO/expired.
    let remaining = deadline
        .and_then(|d| d.checked_duration_since(tokio::time::Instant::now()))
        .unwrap_or(Duration::ZERO);
    let second_deadline = tokio::time::Instant::now().checked_add(remaining);
    let second_wait = if remaining == Duration::ZERO {
        // Observe once without waiting.
        match tokio::time::timeout(Duration::from_millis(0), child.wait()).await {
            Ok(result) => result,
            Err(_) => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "zero close budget expired",
            )),
        }
    } else if let Some(deadline) = second_deadline {
        wait_until(&mut child, deadline).await
    } else {
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "remaining budget unrepresentable",
        ))
    };
    if let Ok(status) = second_wait {
        witness_reaped(status);
        drop(stderr_counter);
        drop(lifeline);
        return CleanupObservation::ReapedAfterKill(status);
    }
    CleanupObservation::Retained(Box::new(RetainedEngine {
        child: Box::new(child),
        lifeline,
        stderr_counter,
    }))
}

/// Waits for the child to exit until the absolute deadline.
async fn wait_until(child: &mut Child, deadline: tokio::time::Instant) -> io::Result<ExitStatus> {
    match tokio::time::timeout_at(deadline, child.wait()).await {
        Ok(result) => result,
        Err(_) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "bounded wait elapsed",
        )),
    }
}

/// Awaits the death of one retained engine once, without a deadline.
///
/// Custody is released only when the wait actually observes an exit; a
/// failed wait hands the whole retained custody back so the owner can park
/// while retaining it.
pub(crate) async fn eventual_wait_once(
    mut retained: Box<RetainedEngine>,
) -> Result<ExitStatus, Box<RetainedEngine>> {
    match retained.child.wait().await {
        Ok(status) => {
            witness_reaped(status);
            Ok(status)
        }
        Err(_) => Err(retained),
    }
}

// ---------------------------------------------------------------------------
// Private cfg(test) bounded phase witnesses.
// ---------------------------------------------------------------------------

/// Witness counts gathered on the actual engine-owner path.
#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct WitnessCounts {
    /// Children successfully spawned.
    pub(crate) spawned: u64,
    /// Termination requests issued by cleanup sequences.
    pub(crate) kills_requested: u64,
    /// Exits actually observed through a completed wait.
    pub(crate) reaps_observed: u64,
    /// How many observed reaps ended with a failure classification.
    pub(crate) watchdog_failures_seen: u64,
    /// Successful control driver joins observed.
    pub(crate) control_driver_joined: u64,
}

#[cfg(test)]
thread_local! {
    static WITNESS_SPAWNED: Cell<u64> = const { Cell::new(0) };
    static WITNESS_KILLS: Cell<u64> = const { Cell::new(0) };
    static WITNESS_REAPS: Cell<u64> = const { Cell::new(0) };
    static WITNESS_WATCHDOG: Cell<u64> = const { Cell::new(0) };
    static WITNESS_CONTROL_DRIVER: Cell<u64> = const { Cell::new(0) };
}

#[cfg(test)]
fn witness_spawned() {
    WITNESS_SPAWNED.with(|c| c.set(c.get() + 1));
}

#[cfg(test)]
fn witness_kill_requested() {
    WITNESS_KILLS.with(|c| c.set(c.get() + 1));
}

#[cfg(test)]
fn witness_reaped(status: ExitStatus) {
    WITNESS_REAPS.with(|c| c.set(c.get() + 1));
    if !status.success() {
        WITNESS_WATCHDOG.with(|c| c.set(c.get() + 1));
    }
}

#[cfg(test)]
pub(crate) fn witness_control_driver_joined() {
    WITNESS_CONTROL_DRIVER.with(|c| c.set(c.get() + 1));
}

/// Resets and reads the phase witnesses (test-only).
#[cfg(test)]
pub(crate) fn reset_witnesses() {
    WITNESS_SPAWNED.with(|c| c.set(0));
    WITNESS_KILLS.with(|c| c.set(0));
    WITNESS_REAPS.with(|c| c.set(0));
    WITNESS_WATCHDOG.with(|c| c.set(0));
    WITNESS_CONTROL_DRIVER.with(|c| c.set(0));
}

/// Reads the current phase witnesses (test-only).
#[cfg(test)]
pub(crate) fn witness_counts() -> WitnessCounts {
    WitnessCounts {
        spawned: WITNESS_SPAWNED.with(std::cell::Cell::get),
        kills_requested: WITNESS_KILLS.with(std::cell::Cell::get),
        reaps_observed: WITNESS_REAPS.with(std::cell::Cell::get),
        watchdog_failures_seen: WITNESS_WATCHDOG.with(std::cell::Cell::get),
        control_driver_joined: WITNESS_CONTROL_DRIVER.with(std::cell::Cell::get),
    }
}

/// Records one exit observed on the success path (test-only).
#[cfg(test)]
pub(crate) fn note_observed_reap_for_tests(status: ExitStatus) {
    witness_reaped(status);
}

#[cfg(not(test))]
fn witness_spawned() {}
#[cfg(not(test))]
fn witness_kill_requested() {}
#[cfg(not(test))]
fn witness_reaped(_status: ExitStatus) {}
