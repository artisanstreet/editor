//! Private process and stream ownership for the engine owner.
//!
//! Everything here serves the single owner task in
//! [`crate::engine_owner::operation`]: exact-child spawning, the sole
//! stdin lifeline writer, count-only stderr draining, and the fixed cleanup
//! sequence that ends in an observed reap or retained custody. The generic
//! recipe uses the caller-supplied absolute engine executable with an explicit
//! empty environment (`env_clear`); the verified configured path below adds
//! only its frozen managed private environment.
//! P3 adds explicit child environment `OPENCODE_PASSWORD=<secret>` and
//! `OPENCODE_SERVER_PASSWORD=` derived from a fresh 32-byte OS secret that
//! is never logged or cloned. No detached per-pipe reader tasks exist; the
//! owner interleaves waits and control signals on one task.

#[cfg(test)]
use std::cell::{Cell, RefCell};
#[cfg(test)]
use std::collections::BTreeMap;
#[cfg(test)]
use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
#[cfg(test)]
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

#[cfg(test)]
use base64::Engine as _;
#[cfg(windows)]
use command_group::AsyncCommandGroup;
#[cfg(not(windows))]
use tokio::process::Child;
use tokio::process::{ChildStderr, ChildStdin, ChildStdout};

use artisan_domain::RootPath;
use artisan_native_engine::VerifiedOpenCode2ProfileLaunch;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

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

/// Internal process custody for one engine launch.
///
/// Windows stores the `command-group` child itself so the Job Object remains
/// the authority for termination and completion. The pipe handles are taken
/// out through the grouped child's non-consuming `inner()` view only so the
/// operation layer can keep its existing stdin/stdout/stderr surface; the
/// grouped child is never converted back into a raw Tokio child.
pub(crate) struct EngineChild {
    #[cfg(windows)]
    inner: command_group::AsyncGroupChild,
    #[cfg(not(windows))]
    inner: Child,
    /// The child stdin pipe, until the owner moves it into `LifelineWriter`.
    pub(crate) stdin: Option<ChildStdin>,
    /// The child stdout pipe, until the owner takes its readiness stream.
    pub(crate) stdout: Option<ChildStdout>,
    /// The child stderr pipe, until the owner moves it into `StderrCounter`.
    pub(crate) stderr: Option<ChildStderr>,
}

impl EngineChild {
    /// Spawns an engine with whole-job custody on Windows and direct-child
    /// custody elsewhere.
    fn spawn(mut command: tokio::process::Command) -> io::Result<Self> {
        #[cfg(windows)]
        {
            let grouped = command
                .group()
                .kill_on_drop(true)
                .creation_flags(CREATE_NO_WINDOW)
                .spawn()?;
            let child = Self::from_grouped(grouped);
            witness_spawned();
            Ok(child)
        }

        #[cfg(not(windows))]
        {
            command.kill_on_drop(true);
            let child = Self::from_direct(command.spawn()?);
            witness_spawned();
            Ok(child)
        }
    }

    #[cfg(windows)]
    fn from_grouped(mut inner: command_group::AsyncGroupChild) -> Self {
        let (stdin, stdout, stderr) = {
            let child = inner.inner();
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        };
        Self {
            inner,
            stdin,
            stdout,
            stderr,
        }
    }

    #[cfg(not(windows))]
    fn from_direct(mut inner: Child) -> Self {
        let stdin = inner.stdin.take();
        let stdout = inner.stdout.take();
        let stderr = inner.stderr.take();
        Self {
            inner,
            stdin,
            stdout,
            stderr,
        }
    }

    /// Waits for the process or whole process group to complete.
    pub(crate) async fn wait(&mut self) -> io::Result<ExitStatus> {
        self.inner.wait().await
    }

    /// Requests termination of the process or whole process group.
    pub(crate) fn start_kill(&mut self) -> io::Result<()> {
        self.inner.start_kill()
    }

    /// Returns the leader process identifier while it remains available.
    #[allow(dead_code)]
    pub(crate) fn id(&self) -> Option<u32> {
        self.inner.id()
    }
}

#[cfg(test)]
const DESCENDANT_SENTINEL_MARKER_ENV: &str = "ARTISAN_ENGINE_OWNER_DESCENDANT_SENTINEL_MARKER";

#[cfg(test)]
thread_local! {
    static DESCENDANT_SENTINEL_MARKER: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// Supplies the next configured fixture launch with its unique marker path.
///
/// The path is held only in the test thread until spawn, then crosses the
/// process boundary solely as the fixture child's explicit environment entry.
#[cfg(test)]
pub(crate) fn set_descendant_sentinel_marker(path: PathBuf) {
    DESCENDANT_SENTINEL_MARKER.with(|marker| {
        drop(marker.replace(Some(path)));
    });
}

#[cfg(test)]
fn take_descendant_sentinel_marker() -> io::Result<PathBuf> {
    DESCENDANT_SENTINEL_MARKER.with(|marker| {
        marker.replace(None).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "descendant sentinel marker unavailable",
            )
        })
    })
}

/// Spawns one engine child from the supplied recipe with the P3 secret.
///
/// All three standard streams are piped, `kill_on_drop(true)` is set so the
/// final `EngineChild` drop is best-effort containment, and on Windows
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
pub(crate) fn spawn_engine(recipe: &LaunchRecipe, secret: &str) -> io::Result<EngineChild> {
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
    EngineChild::spawn(command)
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
) -> io::Result<EngineChild> {
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
    if scenario == "descendant_holds_sentinel" {
        command.env(
            DESCENDANT_SENTINEL_MARKER_ENV,
            take_descendant_sentinel_marker()?,
        );
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    EngineChild::spawn(command)
}

const CONFIGURED_PROFILE_DIRECTORIES: [&str; 5] = ["config", "cache", "data", "state", "tmp"];
const MAX_MANAGED_CONFIG_BYTES: usize = 16 * 1024;

struct ConfiguredProfileEnvironment {
    config: PathBuf,
    cache: PathBuf,
    data: PathBuf,
    state: PathBuf,
    temp: PathBuf,
    config_content: String,
}

impl ConfiguredProfileEnvironment {
    fn prepare(profile_home: &Path) -> io::Result<Self> {
        artisan_native_engine::validate_private_directory(profile_home)
            .map_err(|_| configured_environment_error())?;

        let config = profile_home.join(CONFIGURED_PROFILE_DIRECTORIES[0]);
        let cache = profile_home.join(CONFIGURED_PROFILE_DIRECTORIES[1]);
        let data = profile_home.join(CONFIGURED_PROFILE_DIRECTORIES[2]);
        let state = profile_home.join(CONFIGURED_PROFILE_DIRECTORIES[3]);
        let temp = profile_home.join(CONFIGURED_PROFILE_DIRECTORIES[4]);
        for directory in [&config, &cache, &data, &state, &temp] {
            artisan_native_engine::ensure_private_directory(directory)
                .map_err(|_| configured_environment_error())?;
        }

        Ok(Self {
            config,
            cache,
            data,
            state,
            temp,
            config_content: managed_config_content()?,
        })
    }

    fn apply(&self, command: &mut tokio::process::Command, secret: &str) {
        command
            .env_clear()
            .env("OPENCODE_CLIENT", "artisan-editor")
            .env("OPENCODE_CONFIG_CONTENT", &self.config_content)
            .env("OPENCODE_CONFIG_DIR", &self.config)
            .env("OPENCODE_CONFIG_PROJECT_DISABLE", "1")
            .env("OPENCODE_DB", self.data.join("opencode.db"))
            .env("OPENCODE_FILEWATCHER_DISABLE", "1")
            .env("OPENCODE_PASSWORD", secret)
            .env("OPENCODE_SERVER_PASSWORD", "")
            .env("TMP", &self.temp)
            .env("TEMP", &self.temp)
            .env("XDG_CACHE_HOME", &self.cache)
            .env("XDG_CONFIG_HOME", &self.config)
            .env("XDG_DATA_HOME", &self.data)
            .env("XDG_STATE_HOME", &self.state);
        #[cfg(windows)]
        if let Some(system_root) = std::env::var_os("SYSTEMROOT") {
            command.env("SYSTEMROOT", system_root);
        }
    }
}

fn configured_environment_error() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "configured profile environment rejected",
    )
}

fn managed_configuration_error() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "managed OpenCode configuration unavailable",
    )
}

fn managed_permission_rule(action: &str, effect: &str) -> serde_json::Value {
    serde_json::json!({
        "action": action,
        "effect": effect,
        "resource": "*",
    })
}

fn managed_config_content() -> io::Result<String> {
    let mut agents = serde_json::Map::new();
    for permission in ["restricted", "autonomous", "unrestricted"] {
        for network_access in [false, true] {
            for web_search_enabled in [false, true] {
                let allowed_network = network_access && permission != "restricted";
                let agent_id = format!(
                    "artisan-v1-{permission}-{}-{}",
                    if allowed_network {
                        "network"
                    } else {
                        "offline"
                    },
                    if web_search_enabled { "web" } else { "no-web" },
                );
                let mut permissions = vec![
                    managed_permission_rule("*", "deny"),
                    managed_permission_rule("read", "allow"),
                    managed_permission_rule("glob", "allow"),
                    managed_permission_rule("grep", "allow"),
                    managed_permission_rule("question", "allow"),
                ];
                if permission != "restricted" {
                    permissions.push(managed_permission_rule("edit", "allow"));
                }
                if allowed_network {
                    permissions.push(managed_permission_rule(
                        "shell",
                        if permission == "unrestricted" {
                            "allow"
                        } else {
                            "ask"
                        },
                    ));
                }
                if web_search_enabled {
                    permissions.push(managed_permission_rule("webfetch", "allow"));
                    permissions.push(managed_permission_rule("websearch", "allow"));
                }
                if permission == "unrestricted" {
                    for action in ["external_directory", "execute", "mcp", "skill"] {
                        permissions.push(managed_permission_rule(action, "allow"));
                    }
                }
                permissions.push(managed_permission_rule("subagent", "deny"));
                drop(agents.insert(
                    agent_id,
                    serde_json::json!({
                        "description": "Artisan-managed OpenCode execution policy.",
                        "hidden": true,
                        "mode": "primary",
                        "permissions": permissions,
                        "system": "Follow Artisan product instructions and session guidance. Never claim that OpenCode permission rules are an operating-system sandbox.",
                    }),
                ));
            }
        }
    }

    let document = serde_json::json!({
        "agents": agents,
        "autoupdate": false,
        "default_agent": "artisan-v1-autonomous-offline-no-web",
        "share": "disabled",
        "warming": false,
    });
    let bytes = serde_json::to_vec(&document).map_err(|_| managed_configuration_error())?;
    if bytes.len() > MAX_MANAGED_CONFIG_BYTES {
        return Err(managed_configuration_error());
    }
    String::from_utf8(bytes).map_err(|_| managed_configuration_error())
}

/// Spawns the exact certified `OpenCode2` profile selected for one durable run.
///
/// Revalidation is deliberately the last authority operation before the
/// child is created.  The capability owns the installation fence for the
/// entire call, so a replacement generation cannot race the command.  The
/// environment is cleared and rebuilt only from the managed private profile
/// roots and the per-process credential supplied by the owner.
pub(crate) fn spawn_configured_engine(
    launch: &VerifiedOpenCode2ProfileLaunch,
    project_root: &RootPath,
    secret: &str,
) -> io::Result<EngineChild> {
    let environment = ConfiguredProfileEnvironment::prepare(launch.profile_home())?;

    let mut command = tokio::process::Command::new(launch.executable_path());
    command
        .current_dir(Path::new(project_root.as_str()))
        .args(["serve", "--stdio", "--port", "0"]);
    environment.apply(&mut command, secret);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    launch
        .revalidate()
        .map_err(|_| io::Error::new(io::ErrorKind::PermissionDenied, "profile rejected"))?;
    EngineChild::spawn(command)
}

/// The taken sole stdin writer kept open for the whole operation.
///
/// The writer is removed from the [`EngineChild`] immediately after spawn so the
/// child's own `wait()` can never close it implicitly; it closes exactly
/// when the owner drops it.
pub(crate) struct LifelineWriter(Option<ChildStdin>);

impl LifelineWriter {
    /// Takes the child's piped stdin as the sole lifeline writer.
    pub(crate) fn take(child: &mut EngineChild) -> Self {
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
    /// The exact direct child or whole-job child, boxed for long-term custody.
    pub(crate) child: Box<EngineChild>,
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
    /// The exact child custody object; kept owned until an observed reap or quarantine.
    pub(crate) child: EngineChild,
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
async fn wait_until(
    child: &mut EngineChild,
    deadline: tokio::time::Instant,
) -> io::Result<ExitStatus> {
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
    /// How many observed reaps ended with the fixture watchdog exit (99).
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
const WATCHDOG_FAILURE_EXIT: i32 = 99;

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
    if status.code() == Some(WATCHDOG_FAILURE_EXIT) {
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
    DESCENDANT_SENTINEL_MARKER.with(|marker| {
        drop(marker.replace(None));
    });
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    static TEST_PATH_COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TestPath {
        path: PathBuf,
    }

    impl TestPath {
        fn new(label: &str) -> Self {
            let counter = TEST_PATH_COUNTER.fetch_add(1, Ordering::Relaxed);
            Self {
                path: std::env::temp_dir().join(format!(
                    "artisan-engine-owner-{label}-{}-{counter}",
                    std::process::id(),
                )),
            }
        }

        fn private(label: &str) -> Self {
            let path = Self::new(label);
            artisan_native_engine::ensure_private_directory(&path.path)
                .expect("test profile home should be private");
            path
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TestPath {
        fn drop(&mut self) {
            let Ok(metadata) = fs::symlink_metadata(&self.path) else {
                return;
            };
            if metadata.is_dir() && !metadata.file_type().is_symlink() {
                let _ = fs::remove_dir_all(&self.path);
            } else {
                let _ = fs::remove_file(&self.path);
            }
        }
    }

    fn assert_profile_rejected(result: io::Result<ConfiguredProfileEnvironment>, path: &Path) {
        let error = result
            .err()
            .expect("unsafe profile home should be rejected");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        let diagnostic = error.to_string();
        assert_eq!(diagnostic, "configured profile environment rejected");
        let path_text = path.to_string_lossy();
        assert!(!diagnostic.contains(path_text.as_ref()));
        assert!(!diagnostic.contains("runtime-secret-redacted"));
        assert!(!path.join("config").exists());
    }

    fn command_environment(
        command: &tokio::process::Command,
    ) -> BTreeMap<OsString, Option<OsString>> {
        command
            .as_std()
            .get_envs()
            .map(|(key, value)| (key.to_os_string(), value.map(std::ffi::OsStr::to_os_string)))
            .collect()
    }

    #[test]
    fn managed_config_is_deterministic_bounded_and_policy_complete() {
        let first = managed_config_content().expect("managed config should serialize");
        let second = managed_config_content().expect("managed config should serialize twice");
        assert_eq!(first, second);
        assert!(first.len() <= MAX_MANAGED_CONFIG_BYTES);

        let document: serde_json::Value =
            serde_json::from_str(&first).expect("managed config should be valid JSON");
        assert_eq!(
            document
                .get("autoupdate")
                .and_then(serde_json::Value::as_bool),
            Some(false)
        );
        assert_eq!(
            document
                .get("default_agent")
                .and_then(|value| value.as_str()),
            Some("artisan-v1-autonomous-offline-no-web")
        );
        assert_eq!(
            document.get("share").and_then(|value| value.as_str()),
            Some("disabled")
        );
        assert_eq!(
            document.get("warming").and_then(serde_json::Value::as_bool),
            Some(false)
        );

        let agents = document
            .get("agents")
            .and_then(|value| value.as_object())
            .expect("managed config should contain agents");
        let expected_ids = [
            "artisan-v1-restricted-offline-no-web",
            "artisan-v1-restricted-offline-web",
            "artisan-v1-autonomous-offline-no-web",
            "artisan-v1-autonomous-offline-web",
            "artisan-v1-autonomous-network-no-web",
            "artisan-v1-autonomous-network-web",
            "artisan-v1-unrestricted-offline-no-web",
            "artisan-v1-unrestricted-offline-web",
            "artisan-v1-unrestricted-network-no-web",
            "artisan-v1-unrestricted-network-web",
        ];
        assert_eq!(agents.len(), expected_ids.len());
        for agent_id in expected_ids {
            let agent = agents
                .get(agent_id)
                .expect("reserved agent should be present");
            assert_eq!(
                agent.get("description").and_then(|value| value.as_str()),
                Some("Artisan-managed OpenCode execution policy.")
            );
            assert_eq!(
                agent.get("hidden").and_then(serde_json::Value::as_bool),
                Some(true)
            );
            assert_eq!(
                agent.get("mode").and_then(|value| value.as_str()),
                Some("primary")
            );
            assert_eq!(
                agent.get("system").and_then(|value| value.as_str()),
                Some(
                    "Follow Artisan product instructions and session guidance. Never claim that OpenCode permission rules are an operating-system sandbox."
                )
            );
            let permissions = agent
                .get("permissions")
                .and_then(|value| value.as_array())
                .expect("reserved agent should contain permission rules");
            assert_eq!(
                permissions
                    .first()
                    .and_then(|rule| rule.get("action"))
                    .and_then(|value| value.as_str()),
                Some("*")
            );
            assert_eq!(
                permissions
                    .first()
                    .and_then(|rule| rule.get("effect"))
                    .and_then(|value| value.as_str()),
                Some("deny")
            );
            assert!(permissions.iter().any(|rule| {
                rule.get("action").and_then(|value| value.as_str()) == Some("subagent")
                    && rule.get("effect").and_then(|value| value.as_str()) == Some("deny")
                    && rule.get("resource").and_then(|value| value.as_str()) == Some("*")
            }));
        }
    }

    fn expected_configured_environment(
        home: &TestPath,
        environment: &ConfiguredProfileEnvironment,
        secret: &str,
    ) -> BTreeMap<OsString, Option<OsString>> {
        let mut expected = BTreeMap::from([
            (
                OsString::from("OPENCODE_CLIENT"),
                Some(OsString::from("artisan-editor")),
            ),
            (
                OsString::from("OPENCODE_CONFIG_CONTENT"),
                Some(OsString::from(environment.config_content.as_str())),
            ),
            (
                OsString::from("OPENCODE_CONFIG_DIR"),
                Some(home.path().join("config").into_os_string()),
            ),
            (
                OsString::from("OPENCODE_CONFIG_PROJECT_DISABLE"),
                Some(OsString::from("1")),
            ),
            (
                OsString::from("OPENCODE_DB"),
                Some(
                    home.path()
                        .join("data")
                        .join("opencode.db")
                        .into_os_string(),
                ),
            ),
            (
                OsString::from("OPENCODE_FILEWATCHER_DISABLE"),
                Some(OsString::from("1")),
            ),
            (
                OsString::from("OPENCODE_PASSWORD"),
                Some(OsString::from(secret)),
            ),
            (
                OsString::from("OPENCODE_SERVER_PASSWORD"),
                Some(OsString::new()),
            ),
            (
                OsString::from("TMP"),
                Some(home.path().join("tmp").into_os_string()),
            ),
            (
                OsString::from("TEMP"),
                Some(home.path().join("tmp").into_os_string()),
            ),
            (
                OsString::from("XDG_CACHE_HOME"),
                Some(home.path().join("cache").into_os_string()),
            ),
            (
                OsString::from("XDG_CONFIG_HOME"),
                Some(home.path().join("config").into_os_string()),
            ),
            (
                OsString::from("XDG_DATA_HOME"),
                Some(home.path().join("data").into_os_string()),
            ),
            (
                OsString::from("XDG_STATE_HOME"),
                Some(home.path().join("state").into_os_string()),
            ),
        ]);
        #[cfg(windows)]
        if let Some(system_root) = std::env::var_os("SYSTEMROOT") {
            expected.insert(OsString::from("SYSTEMROOT"), Some(system_root));
        }
        expected
    }

    #[test]
    fn configured_environment_projects_exact_allowlist_without_ambient_values() {
        let home = TestPath::private("environment");
        let environment = ConfiguredProfileEnvironment::prepare(home.path())
            .expect("private profile environment should prepare");
        let secret = "runtime-secret-redacted";
        let profile_path = home.path().to_string_lossy().into_owned();
        let mut command = tokio::process::Command::new("not-spawned");
        command
            .env("PATH", "ambient-path")
            .env("HOME", "ambient-home")
            .env("USERPROFILE", "ambient-user-profile")
            .env("ARTISAN_AUTH_TOKEN", "ambient-provider-secret");
        environment.apply(&mut command, secret);

        let expected = expected_configured_environment(&home, &environment, secret);

        let actual = command_environment(&command);
        assert_eq!(actual, expected);
        for key in [
            "PATH",
            "HOME",
            "USERPROFILE",
            "ARTISAN_AUTH_TOKEN",
            "OPENCODE_AUTH_TOKEN",
        ] {
            assert!(
                !actual
                    .keys()
                    .any(|actual_key| actual_key == OsStr::new(key))
            );
        }
        assert!(!environment.config_content.contains(secret));
        assert!(!environment.config_content.contains(profile_path.as_str()));

        let diagnostic_path = TestPath::new("diagnostic-secret-path");
        assert_profile_rejected(
            ConfiguredProfileEnvironment::prepare(diagnostic_path.path()),
            diagnostic_path.path(),
        );
    }

    #[test]
    fn configured_environment_creates_valid_private_directories_idempotently() {
        let home = TestPath::private("directories");
        let environment = ConfiguredProfileEnvironment::prepare(home.path())
            .expect("private profile environment should prepare");
        for directory_name in CONFIGURED_PROFILE_DIRECTORIES {
            let directory = home.path().join(directory_name);
            assert!(directory.is_dir());
            assert!(artisan_native_engine::validate_private_directory(&directory).is_ok());
        }

        let marker = home.path().join("data").join("preserve-me");
        fs::write(&marker, b"existing profile data").expect("marker should be writable");
        let second = ConfiguredProfileEnvironment::prepare(home.path())
            .expect("preparing an existing profile should be idempotent");
        assert_eq!(
            fs::read(marker).expect("marker should remain"),
            b"existing profile data"
        );
        assert_eq!(environment.config_content, second.config_content);
    }

    #[test]
    fn configured_environment_rejects_missing_profile_home() {
        let missing = TestPath::new("missing");
        assert_profile_rejected(
            ConfiguredProfileEnvironment::prepare(missing.path()),
            missing.path(),
        );
    }

    #[cfg(unix)]
    #[test]
    fn configured_environment_rejects_non_private_profile_home() {
        use std::os::unix::fs::PermissionsExt;

        let home = TestPath::private("non-private");
        fs::set_permissions(home.path(), fs::Permissions::from_mode(0o755))
            .expect("test profile permissions should change");
        assert_profile_rejected(
            ConfiguredProfileEnvironment::prepare(home.path()),
            home.path(),
        );
    }

    #[cfg(unix)]
    #[test]
    fn configured_environment_rejects_symlinked_profile_home() {
        use std::os::unix::fs::symlink;

        let target = TestPath::private("symlink-target");
        let link = TestPath::new("symlink");
        symlink(target.path(), link.path()).expect("test symlink should be creatable");
        assert_profile_rejected(
            ConfiguredProfileEnvironment::prepare(link.path()),
            link.path(),
        );
    }

    #[test]
    fn configured_environment_rejects_non_directory_profile_home() {
        let file = TestPath::new("non-directory");
        fs::write(file.path(), b"not a directory").expect("test file should be writable");
        assert_profile_rejected(
            ConfiguredProfileEnvironment::prepare(file.path()),
            file.path(),
        );
    }
}
