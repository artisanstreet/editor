use std::{
    env,
    ffi::{OsStr, OsString},
    fmt,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde::{Deserialize, Deserializer, Serialize};

use crate::{
    CliError, Result,
    credentials::ForgeCredentialPaths,
    error::{ForgeTermination, io},
    instance::NativeInstanceConfig,
    manifest::InstallationManifest,
};

const FORGE_START_TIMEOUT: Duration = Duration::from_secs(30);
const FORGE_SHUTDOWN_GRACE_MAX: Duration = Duration::from_secs(2);
const FORGE_READY_INTERVAL: Duration = Duration::from_millis(100);
const MAX_READINESS_BYTES: usize = 4096;
const READY_SCHEMA: &str = "artisan-forge-ready-v1";

#[derive(Clone, Eq, PartialEq)]
pub struct ForgeLaunchSpec {
    executable: PathBuf,
    argv: Vec<OsString>,
    readiness_path: PathBuf,
}

impl fmt::Debug for ForgeLaunchSpec {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ForgeLaunchSpec")
            .field("argv_count", &self.argv.len())
            .finish_non_exhaustive()
    }
}

impl ForgeLaunchSpec {
    pub fn new(
        manifest: &InstallationManifest,
        config: &NativeInstanceConfig,
        credentials: &ForgeCredentialPaths,
    ) -> Result<Self> {
        validate_credential_manifest(config, credentials)?;

        Ok(Self {
            executable: manifest.forge_executable(),
            argv: native_argv(
                config,
                credentials.certificate_paths(),
                credentials.private_key_path(),
                credentials.capability_path(),
            ),
            readiness_path: config.readiness_path().to_path_buf(),
        })
    }

    pub fn executable(&self) -> &Path {
        &self.executable
    }

    pub fn argv(&self) -> &[OsString] {
        &self.argv
    }

    pub fn readiness_path(&self) -> &Path {
        &self.readiness_path
    }
}

pub(crate) fn validate_credential_manifest(
    config: &NativeInstanceConfig,
    credentials: &ForgeCredentialPaths,
) -> Result<()> {
    let configured_manifest = config.credentials_manifest().to_path_buf();
    let credential_manifest = credentials.manifest_path().to_path_buf();
    if configured_manifest != credential_manifest {
        return Err(CliError::CredentialManifestMismatch {
            configured: configured_manifest,
            credentials: credential_manifest,
        });
    }
    Ok(())
}

/// The non-secret receipt a native Forge publishes after its listener and
/// certificate identity are ready.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ForgeReadiness {
    schema: String,
    endpoint: String,
    certificate_sha256: String,
    pid: std::num::NonZeroU32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ForgeReadinessFile {
    schema: String,
    endpoint: String,
    certificate_sha256: String,
    pid: u32,
}

impl ForgeReadiness {
    pub fn new(
        schema: impl Into<String>,
        endpoint: impl Into<String>,
        certificate_sha256: impl Into<String>,
        pid: u32,
    ) -> Result<Self> {
        let schema = schema.into();
        if schema != READY_SCHEMA {
            return Err(CliError::InvalidForgeReadiness {
                reason: "schema is not artisan-forge-ready-v1",
            });
        }

        let endpoint = endpoint.into();
        if !is_exact_loopback_endpoint(&endpoint) {
            return Err(CliError::InvalidForgeReadiness {
                reason: "endpoint is not an IPv4 127.0.0.1 address with a nonzero port",
            });
        }

        let certificate_sha256 = certificate_sha256.into();
        if !is_sha256_hex(&certificate_sha256) {
            return Err(CliError::InvalidForgeReadiness {
                reason: "certificate SHA-256 is not exactly 64 ASCII hex characters",
            });
        }

        let pid = std::num::NonZeroU32::new(pid).ok_or(CliError::InvalidForgeReadiness {
            reason: "PID must be nonzero",
        })?;

        Ok(Self {
            schema,
            endpoint,
            certificate_sha256,
            pid,
        })
    }

    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_READINESS_BYTES {
            return Err(CliError::InvalidForgeReadiness {
                reason: "receipt exceeds its size bound",
            });
        }
        serde_json::from_slice(bytes).map_err(|_| CliError::InvalidForgeReadiness {
            reason: "receipt JSON is malformed or has an unsupported shape",
        })
    }

    pub fn schema(&self) -> &str {
        &self.schema
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn certificate_sha256(&self) -> &str {
        &self.certificate_sha256
    }

    pub fn pid(&self) -> u32 {
        self.pid.get()
    }
}

impl<'de> Deserialize<'de> for ForgeReadiness {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let file = ForgeReadinessFile::deserialize(deserializer)?;
        Self::new(
            file.schema,
            file.endpoint,
            file.certificate_sha256,
            file.pid,
        )
        .map_err(|error| <D::Error as serde::de::Error>::custom(error.to_string()))
    }
}

fn is_exact_loopback_endpoint(endpoint: &str) -> bool {
    let Some(port_text) = endpoint.strip_prefix("127.0.0.1:") else {
        return false;
    };
    if port_text.is_empty()
        || (port_text.len() > 1 && port_text.starts_with('0'))
        || !port_text.bytes().all(|byte| byte.is_ascii_digit())
    {
        return false;
    }
    port_text.parse::<u16>().is_ok_and(|port| port != 0)
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// The result a status caller can safely report without treating readiness as
/// authenticated lifecycle or busy state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ForgeReadinessStatus {
    Missing,
    Invalid,
    Ready(ForgeReadiness),
}

fn native_argv(
    config: &NativeInstanceConfig,
    certificate_paths: &[PathBuf],
    private_key_path: &Path,
    capability_path: &Path,
) -> Vec<OsString> {
    let mut argv = Vec::with_capacity(38 + certificate_paths.len() * 2);
    append_path(&mut argv, "--database", config.database_path());
    append_path(&mut argv, "--custody", config.custody_path());
    for certificate_path in certificate_paths {
        append_path(&mut argv, "--certificate-der", certificate_path);
    }
    append_path(&mut argv, "--private-key-der", private_key_path);
    append_path(&mut argv, "--bootstrap-capability", capability_path);
    append_path(&mut argv, "--ready-file", config.readiness_path());
    append_number(
        &mut argv,
        "--admission-timeout-ms",
        config.listener().admission_timeout_ms(),
    );
    append_number(
        &mut argv,
        "--handshake-timeout-ms",
        config.listener().handshake_timeout_ms(),
    );
    append_number(
        &mut argv,
        "--request-timeout-ms",
        config.listener().request_timeout_ms(),
    );
    append_number(
        &mut argv,
        "--drain-timeout-ms",
        config.listener().drain_timeout_ms(),
    );
    append_number(
        &mut argv,
        "--admission-capacity",
        u64::from(config.listener().admission_capacity().get()),
    );
    append_number(
        &mut argv,
        "--requests-per-connection",
        u64::from(config.listener().requests_per_connection().get()),
    );
    append_number(
        &mut argv,
        "--native-run-claim-lease-ms",
        config.native_run().claim_lease_ms(),
    );
    append_number(
        &mut argv,
        "--native-run-poll-interval-ms",
        config.native_run().poll_interval_ms(),
    );
    append_number(
        &mut argv,
        "--native-run-retry-backoff-ms",
        config.native_run().retry_backoff_ms(),
    );
    append_number(
        &mut argv,
        "--native-run-shutdown-budget-ms",
        config.native_run().shutdown_budget_ms(),
    );
    append_number(
        &mut argv,
        "--native-run-queue-capacity",
        u64::from(config.native_run().queue_capacity().get()),
    );
    append_number(
        &mut argv,
        "--native-run-max-command-retries",
        u64::from(config.native_run().max_command_retries().get()),
    );
    append_text(
        &mut argv,
        "--native-run-prompt-delivery",
        config.native_run().prompt_delivery(),
    );
    append_number(
        &mut argv,
        "--native-run-stream-after",
        config.native_run().stream_after(),
    );
    argv
}

fn append_path(argv: &mut Vec<OsString>, option: &str, path: &Path) {
    argv.push(OsString::from(option));
    argv.push(path.as_os_str().to_os_string());
}

fn append_number(argv: &mut Vec<OsString>, option: &str, value: u64) {
    argv.push(OsString::from(option));
    argv.push(value.to_string().into());
}

fn append_text(argv: &mut Vec<OsString>, option: &str, value: &str) {
    argv.push(OsString::from(option));
    argv.push(OsString::from(value));
}

fn forge_command(spec: &ForgeLaunchSpec) -> Command {
    let mut command = Command::new(spec.executable());
    command.args(spec.argv());
    configure_native_environment(&mut command);
    command
}

fn configure_native_environment(command: &mut Command) {
    configure_environment(command, env::vars_os());
}

fn configure_environment<I>(command: &mut Command, variables: I)
where
    I: IntoIterator<Item = (OsString, OsString)>,
{
    command.env_clear();
    for (key, value) in variables {
        if !is_forbidden_environment_key(&key) {
            command.env(key, value);
        }
    }
}

fn is_forbidden_environment_key(key: &OsStr) -> bool {
    starts_with_ascii_case_insensitive(key, b"ARTISAN_")
        || starts_with_ascii_case_insensitive(key, b"NODE")
        || starts_with_ascii_case_insensitive(key, b"ELECTRON")
        || key
            .as_encoded_bytes()
            .eq_ignore_ascii_case(b"CODEX_SQLITE_HOME")
}

fn starts_with_ascii_case_insensitive(value: &OsStr, prefix: &[u8]) -> bool {
    value
        .as_encoded_bytes()
        .get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

fn ensure_forge_executable(spec: &ForgeLaunchSpec) -> Result<()> {
    if spec.executable().is_file() {
        return Ok(());
    }
    Err(CliError::Installation(format!(
        "Forge binary is missing at {}",
        spec.executable().display()
    )))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StartResult {
    AlreadyRunning,
    Spawned { pid: u32 },
    ForegroundExited,
}

const OWNED_FORGE_ALREADY_RUNNING: &str =
    "refusing to adopt an already-running Forge without process ownership";
const OWNED_FORGE_START_FAILURE: &str = "owned Forge process operation failed during startup";
const OWNED_FORGE_READINESS_FAILURE: &str = "owned Forge readiness operation failed";
const OWNED_FORGE_SHUTDOWN_FAILURE: &str = "could not confirm owned Forge shutdown";

#[derive(Clone, Debug, Eq, PartialEq)]
enum OwnedStartDecision {
    RefuseAlreadyRunning,
    Spawn {
        prior_readiness: Option<ReadinessFileSnapshot>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OwnedProcessStartInfo {
    QueryFailed,
    Missing,
    MissingStartTime,
    Present(u64),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OwnedProcessIdentity {
    Match,
    FailClosed,
}

fn owned_start_decision<F>(
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

fn forge_owned_command_with_environment<I>(
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

fn owned_process_identity(
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

fn owned_process_identity_matches(
    expected_start_time: u64,
    current: OwnedProcessStartInfo,
) -> bool {
    owned_process_identity(expected_start_time, current) == OwnedProcessIdentity::Match
}

fn owned_readiness_candidate<F>(
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

fn owned_already_running_failure() -> CliError {
    CliError::Unsupported(OWNED_FORGE_ALREADY_RUNNING.to_owned())
}

fn owned_shutdown_failure() -> CliError {
    CliError::Io {
        context: "shutdown Forge",
        source: std::io::Error::other(OWNED_FORGE_SHUTDOWN_FAILURE),
    }
}

fn forge_startup_outcome_error(outcome: processkit::Outcome) -> CliError {
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

fn clamp_shutdown_grace(requested_grace: Duration) -> Duration {
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
    process: processkit::RunningProcess,
    pid: u32,
    readiness: ForgeReadiness,
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

pub fn start(spec: &ForgeLaunchSpec, foreground: bool) -> Result<StartResult> {
    start_until(spec, foreground, Instant::now() + FORGE_START_TIMEOUT)
}

pub fn start_until(
    spec: &ForgeLaunchSpec,
    foreground: bool,
    readiness_deadline: Instant,
) -> Result<StartResult> {
    if foreground {
        start_foreground(spec)
    } else {
        spawn_background_forge(spec, readiness_deadline)
    }
}

fn start_foreground(spec: &ForgeLaunchSpec) -> Result<StartResult> {
    ensure_forge_executable(spec)?;
    let status = forge_command(spec).status().map_err(io("start Forge"))?;
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

#[derive(Clone, Debug, Eq, PartialEq)]
struct ReadinessFileSnapshot {
    identity: Option<ReadinessFileIdentity>,
    bytes: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ReadinessFileIdentity {
    first: u64,
    second: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ReadinessFileRead {
    Missing,
    Invalid,
    Present(ReadinessFileSnapshot),
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum BackgroundStartDecision {
    AlreadyRunning,
    Spawn {
        prior_readiness: Option<ReadinessFileSnapshot>,
    },
}

fn background_start_decision<F>(
    existing: ReadinessFileRead,
    expected_executable: &Path,
    resolve_executable: F,
) -> BackgroundStartDecision
where
    F: FnOnce(u32) -> Option<PathBuf>,
{
    match existing {
        ReadinessFileRead::Present(snapshot) => {
            let matches_live_forge =
                ForgeReadiness::from_json(&snapshot.bytes).is_ok_and(|readiness| {
                    readiness_matches_process(&readiness, expected_executable, resolve_executable)
                });
            if matches_live_forge {
                BackgroundStartDecision::AlreadyRunning
            } else {
                BackgroundStartDecision::Spawn {
                    prior_readiness: Some(snapshot),
                }
            }
        }
        ReadinessFileRead::Missing | ReadinessFileRead::Invalid => BackgroundStartDecision::Spawn {
            prior_readiness: None,
        },
    }
}

fn read_readiness_file(path: &Path) -> ReadinessFileRead {
    let path_metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return ReadinessFileRead::Missing;
        }
        Err(_) => return ReadinessFileRead::Invalid,
    };
    if !is_safe_readiness_file(&path_metadata) {
        return ReadinessFileRead::Invalid;
    }

    let Ok(before_file) = File::open(path) else {
        return ReadinessFileRead::Invalid;
    };
    let Ok(before_metadata) = before_file.metadata() else {
        return ReadinessFileRead::Invalid;
    };
    if !is_safe_readiness_file(&before_metadata) {
        return ReadinessFileRead::Invalid;
    }
    let Some(before_identity) = readiness_file_identity(&before_file) else {
        return ReadinessFileRead::Invalid;
    };
    drop(before_file);

    let Ok(mut file) = File::open(path) else {
        return ReadinessFileRead::Invalid;
    };
    let Ok(opened_metadata) = file.metadata() else {
        return ReadinessFileRead::Invalid;
    };
    let Some(opened_identity) = readiness_file_identity(&file) else {
        return ReadinessFileRead::Invalid;
    };
    if before_identity != opened_identity || !is_safe_readiness_file(&opened_metadata) {
        return ReadinessFileRead::Invalid;
    }

    let mut bytes = Vec::with_capacity(MAX_READINESS_BYTES.min(256));
    if file
        .by_ref()
        .take((MAX_READINESS_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return ReadinessFileRead::Invalid;
    }
    let Ok(final_metadata) = file.metadata() else {
        return ReadinessFileRead::Invalid;
    };
    let Some(final_identity) = readiness_file_identity(&file) else {
        return ReadinessFileRead::Invalid;
    };
    if opened_identity != final_identity || !is_safe_readiness_file(&final_metadata) {
        return ReadinessFileRead::Invalid;
    }
    let Ok(final_path_metadata) = fs::symlink_metadata(path) else {
        return ReadinessFileRead::Invalid;
    };
    if !is_safe_readiness_file(&final_path_metadata) {
        return ReadinessFileRead::Invalid;
    }
    let Ok(final_file) = File::open(path) else {
        return ReadinessFileRead::Invalid;
    };
    let Ok(final_file_metadata) = final_file.metadata() else {
        return ReadinessFileRead::Invalid;
    };
    if !is_safe_readiness_file(&final_file_metadata)
        || readiness_file_identity(&final_file) != Some(final_identity)
    {
        return ReadinessFileRead::Invalid;
    }

    ReadinessFileRead::Present(ReadinessFileSnapshot {
        identity: Some(final_identity),
        bytes,
    })
}

fn is_safe_readiness_file(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;

        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return false;
        }
    }
    true
}

fn readiness_file_identity(file: &File) -> Option<ReadinessFileIdentity> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        let metadata = file.metadata().ok()?;
        return Some(ReadinessFileIdentity {
            first: metadata.dev(),
            second: metadata.ino(),
        });
    }
    #[cfg(windows)]
    {
        let information =
            winapi_util::file::information(winapi_util::HandleRef::from_file(file)).ok()?;
        let volume = information.volume_serial_number();
        let index = information.file_index();
        if volume == 0 && index == 0 {
            return None;
        }
        Some(ReadinessFileIdentity {
            first: volume,
            second: index,
        })
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = file;
        None
    }
}

fn readiness_file_replaced(
    prior: Option<&ReadinessFileSnapshot>,
    current: &ReadinessFileSnapshot,
) -> bool {
    let Some(current_identity) = current.identity else {
        return false;
    };
    match prior {
        None => true,
        Some(prior) => prior
            .identity
            .is_some_and(|prior_identity| prior_identity != current_identity),
    }
}

fn readiness_matches_process<F>(
    readiness: &ForgeReadiness,
    expected_executable: &Path,
    resolve_executable: F,
) -> bool
where
    F: FnOnce(u32) -> Option<PathBuf>,
{
    readiness_matches_child(
        readiness,
        readiness.pid(),
        expected_executable,
        resolve_executable,
    )
}

fn readiness_matches_child<F>(
    readiness: &ForgeReadiness,
    child_pid: u32,
    expected_executable: &Path,
    resolve_executable: F,
) -> bool
where
    F: FnOnce(u32) -> Option<PathBuf>,
{
    readiness.pid() == child_pid
        && resolve_executable(child_pid)
            .is_some_and(|actual| same_executable(&actual, expected_executable))
}

fn same_executable(left: &Path, right: &Path) -> bool {
    #[cfg(windows)]
    {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

#[cfg(windows)]
fn process_executable(pid: u32) -> Option<PathBuf> {
    let system_root = env::var_os("SystemRoot")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())?;
    let powershell = system_root
        .join("System32")
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("powershell.exe");
    if !powershell.is_file() {
        return None;
    }

    let filter = format!("ProcessId = {pid}");
    let output = Command::new(powershell)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
        ])
        .arg(format!(
            "(Get-CimInstance -ClassName Win32_Process -Filter '{filter}').ExecutablePath"
        ))
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    let path = stdout
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    let path = PathBuf::from(path);
    path.is_absolute().then_some(path)
}

#[cfg(unix)]
fn process_executable(pid: u32) -> Option<PathBuf> {
    fs::read_link(format!("/proc/{pid}/exe")).ok()
}

#[cfg(not(any(unix, windows)))]
fn process_executable(_: u32) -> Option<PathBuf> {
    None
}

trait ChildProbe {
    fn pid(&self) -> u32;
    fn poll_termination(&mut self) -> Result<Option<ForgeTermination>>;
}

impl ChildProbe for Child {
    fn pid(&self) -> u32 {
        self.id()
    }

    fn poll_termination(&mut self) -> Result<Option<ForgeTermination>> {
        self.try_wait()
            .map(|status| status.map(|status| ForgeTermination::from_exit_status(&status)))
            .map_err(io("poll Forge startup"))
    }
}

#[allow(clippy::too_many_arguments)]
fn wait_for_readiness_with<C, ReadReceipt, ResolveExecutable, Sleep>(
    child: &mut C,
    expected_executable: &Path,
    readiness_path: &Path,
    prior_readiness: Option<&ReadinessFileSnapshot>,
    deadline: Instant,
    mut read_receipt: ReadReceipt,
    resolve_executable: ResolveExecutable,
    mut sleep: Sleep,
) -> Result<StartResult>
where
    C: ChildProbe,
    ReadReceipt: FnMut(&Path) -> ReadinessFileRead,
    ResolveExecutable: Fn(u32) -> Option<PathBuf>,
    Sleep: FnMut(Instant, Duration),
{
    let child_pid = child.pid();
    loop {
        if let Some(termination) = child.poll_termination()? {
            return Err(CliError::ForgeTerminated { termination });
        }
        if Instant::now() >= deadline {
            return readiness_timeout_or_child_exit(child);
        }

        if let ReadinessFileRead::Present(snapshot) = read_receipt(readiness_path)
            && readiness_file_replaced(prior_readiness, &snapshot)
            && let Ok(readiness) = ForgeReadiness::from_json(&snapshot.bytes)
            && readiness_matches_child(
                &readiness,
                child_pid,
                expected_executable,
                &resolve_executable,
            )
        {
            if Instant::now() >= deadline {
                return readiness_timeout_or_child_exit(child);
            }
            if let Some(termination) = child.poll_termination()? {
                return Err(CliError::ForgeTerminated { termination });
            }
            return Ok(StartResult::Spawned { pid: child_pid });
        }

        if Instant::now() >= deadline {
            return readiness_timeout_or_child_exit(child);
        }
        sleep(deadline, FORGE_READY_INTERVAL);
    }
}

fn readiness_timeout_or_child_exit<C: ChildProbe>(child: &mut C) -> Result<StartResult> {
    if let Some(termination) = child.poll_termination()? {
        Err(CliError::ForgeTerminated { termination })
    } else {
        Err(CliError::ForgeReadinessTimeout)
    }
}

pub fn readiness_status(readiness_path: &Path, expected_executable: &Path) -> ForgeReadinessStatus {
    match read_readiness_file(readiness_path) {
        ReadinessFileRead::Missing => ForgeReadinessStatus::Missing,
        ReadinessFileRead::Invalid => ForgeReadinessStatus::Invalid,
        ReadinessFileRead::Present(snapshot) => {
            let Ok(readiness) = ForgeReadiness::from_json(&snapshot.bytes) else {
                return ForgeReadinessStatus::Invalid;
            };
            if readiness_matches_process(&readiness, expected_executable, process_executable) {
                ForgeReadinessStatus::Ready(readiness)
            } else {
                ForgeReadinessStatus::Invalid
            }
        }
    }
}

fn poll_delay(deadline: Instant, interval: Duration, now: Instant) -> Option<Duration> {
    deadline
        .checked_duration_since(now)
        .filter(|remaining| !remaining.is_zero())
        .map(|remaining| remaining.min(interval))
}

fn sleep_until(deadline: Instant, interval: Duration) {
    if let Some(delay) = poll_delay(deadline, interval, Instant::now()) {
        thread::sleep(delay);
    }
}

#[cfg(target_os = "windows")]
fn detach(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    // A hidden console rather than none (`DETACHED_PROCESS`). The Forge runs
    // console children constantly — git for every project reading, PowerShell
    // for file ACLs — and a child of a console-less parent allocates its own
    // visible console, which flashed a terminal window over the editor per
    // call. Children inherit this hidden console instead, so nothing paints.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
}

#[cfg(not(target_os = "windows"))]
fn detach(_: &mut Command) {
    // std has no portable daemon/session API. Redirected stdio still makes the
    // child independent of this terminal; installers may add a service manager.
}

#[cfg(test)]
mod tests {
    use std::{
        ffi::{OsStr, OsString},
        num::NonZeroU32,
        path::{Path, PathBuf},
        time::{Duration, Instant},
    };

    use super::{
        BackgroundStartDecision, ChildProbe, Command, FORGE_SHUTDOWN_GRACE_MAX, ForgeLaunchSpec,
        ForgeProcessLease, ForgeReadiness, OwnedProcessIdentity, OwnedProcessStartInfo,
        OwnedStartDecision, ReadinessFileIdentity, ReadinessFileRead, ReadinessFileSnapshot,
        StartResult, background_start_decision, clamp_shutdown_grace, configure_environment,
        forge_owned_command_with_environment, forge_startup_outcome_error,
        is_forbidden_environment_key, native_argv, owned_already_running_failure,
        owned_process_identity, owned_process_identity_matches, owned_readiness_candidate,
        owned_shutdown_failure, owned_start_decision, poll_delay, readiness_file_replaced,
        readiness_matches_child, readiness_matches_process, wait_for_readiness_with,
    };

    use crate::{
        CliError,
        credentials::ForgeCredentialPaths,
        error::{ForgeExitCode, ForgeTermination},
        instance::{
            NativeInstanceConfig, NativeListenerConfig, NativeRunConfig, NativeRunConfigInput,
        },
        manifest::InstallationManifest,
    };

    fn test_native_config(home: &Path, credentials_manifest: &Path) -> NativeInstanceConfig {
        NativeInstanceConfig::new(
            home.join("data").join("forge.sqlite3"),
            home.join("custody").join("forge.lock"),
            home.join("readiness").join("forge.json"),
            credentials_manifest.to_path_buf(),
            NativeListenerConfig::new(
                11,
                12,
                13,
                14,
                NonZeroU32::new(3).unwrap(),
                NonZeroU32::new(4).unwrap(),
            ),
            NativeRunConfig::new(NativeRunConfigInput {
                claim_lease_ms: 15,
                poll_interval_ms: 16,
                retry_backoff_ms: 17,
                shutdown_budget_ms: 18,
                queue_capacity: 5,
                max_command_retries: 6,
                prompt_delivery: "queue".to_owned(),
                stream_after: 7,
            })
            .unwrap(),
        )
        .unwrap()
    }

    fn test_manifest() -> InstallationManifest {
        InstallationManifest {
            activation_state: "active".into(),
            finalization_state: Some("complete".into()),
            active_version: Some("1.2.3".into()),
            install_root: if cfg!(windows) {
                PathBuf::from(r"C:\Users\Ada\Artisan Street")
            } else {
                PathBuf::from("/opt/Artisan Street")
            },
            permanent_ae_path: None,
        }
    }

    fn test_launch_spec() -> ForgeLaunchSpec {
        let home = tempfile::tempdir().unwrap();
        let credentials = ForgeCredentialPaths::from_home(home.path()).unwrap();
        let config = test_native_config(home.path(), credentials.manifest_path());
        ForgeLaunchSpec::new(&test_manifest(), &config, &credentials).unwrap()
    }

    #[test]
    fn native_launch_spec_uses_the_versioned_forge_and_exact_argv() {
        let spec = test_launch_spec();
        assert_eq!(spec.executable, test_manifest().forge_executable());
        assert_eq!(spec.argv.len(), 40);
        assert_eq!(spec.argv[0], OsString::from("--database"));
        assert_eq!(spec.argv[2], OsString::from("--custody"));
        assert_eq!(spec.argv[4], OsString::from("--certificate-der"));
        assert_eq!(spec.argv[6], OsString::from("--private-key-der"));
        assert_eq!(spec.argv[8], OsString::from("--bootstrap-capability"));
        assert_eq!(spec.argv[10], OsString::from("--ready-file"));
        assert_eq!(spec.argv[12], OsString::from("--admission-timeout-ms"));
        assert_eq!(spec.argv[14], OsString::from("--handshake-timeout-ms"));
        assert_eq!(spec.argv[16], OsString::from("--request-timeout-ms"));
        assert_eq!(spec.argv[18], OsString::from("--drain-timeout-ms"));
        assert_eq!(spec.argv[20], OsString::from("--admission-capacity"));
        assert_eq!(spec.argv[21], OsString::from("3"));
        assert_eq!(spec.argv[22], OsString::from("--requests-per-connection"));
        assert_eq!(spec.argv[23], OsString::from("4"));
        assert_eq!(spec.argv[24], OsString::from("--native-run-claim-lease-ms"));
        assert_eq!(spec.argv[25], OsString::from("15"));
        assert_eq!(
            spec.argv[26],
            OsString::from("--native-run-poll-interval-ms")
        );
        assert_eq!(spec.argv[27], OsString::from("16"));
        assert_eq!(
            spec.argv[28],
            OsString::from("--native-run-retry-backoff-ms")
        );
        assert_eq!(spec.argv[29], OsString::from("17"));
        assert_eq!(
            spec.argv[30],
            OsString::from("--native-run-shutdown-budget-ms")
        );
        assert_eq!(spec.argv[31], OsString::from("18"));
        assert_eq!(spec.argv[32], OsString::from("--native-run-queue-capacity"));
        assert_eq!(spec.argv[33], OsString::from("5"));
        assert_eq!(
            spec.argv[34],
            OsString::from("--native-run-max-command-retries")
        );
        assert_eq!(spec.argv[35], OsString::from("6"));
        assert_eq!(
            spec.argv[36],
            OsString::from("--native-run-prompt-delivery")
        );
        assert_eq!(spec.argv[37], OsString::from("queue"));
        assert_eq!(spec.argv[38], OsString::from("--native-run-stream-after"));
        assert_eq!(spec.argv[39], OsString::from("7"));
    }

    #[test]
    fn launch_spec_debug_redacts_credential_paths_and_argv_values() {
        let spec = ForgeLaunchSpec {
            executable: PathBuf::from("FORGE-EXECUTABLE-SECRET"),
            argv: vec![
                OsString::from("--private-key-der"),
                OsString::from("ARGV-CREDENTIAL-SECRET"),
            ],
            readiness_path: PathBuf::from("READINESS-PATH-SECRET"),
        };

        let debug = format!("{spec:?}");
        assert!(debug.contains("ForgeLaunchSpec"));
        assert!(debug.contains("argv_count: 2"));
        for secret in [
            "FORGE-EXECUTABLE-SECRET",
            "ARGV-CREDENTIAL-SECRET",
            "READINESS-PATH-SECRET",
        ] {
            assert!(!debug.contains(secret), "debug leaked {secret}");
        }
    }

    #[test]
    fn native_argv_preserves_repeated_certificate_order_and_os_paths() {
        let home = tempfile::tempdir().unwrap();
        let credentials = ForgeCredentialPaths::from_home(home.path()).unwrap();
        let config = test_native_config(home.path(), credentials.manifest_path());
        let certificates = vec![
            home.path().join("Artisan Street").join("leaf.der"),
            home.path().join("Artisan Street").join("intermediate.der"),
        ];
        let argv = native_argv(
            &config,
            &certificates,
            credentials.private_key_path(),
            credentials.capability_path(),
        );

        let mut expected = Vec::new();
        for (option, path) in [
            ("--database", config.database_path()),
            ("--custody", config.custody_path()),
        ] {
            expected.push(OsString::from(option));
            expected.push(path.as_os_str().to_os_string());
        }
        for certificate in &certificates {
            expected.push(OsString::from("--certificate-der"));
            expected.push(certificate.as_os_str().to_os_string());
        }
        for (option, path) in [
            ("--private-key-der", credentials.private_key_path()),
            ("--bootstrap-capability", credentials.capability_path()),
            ("--ready-file", config.readiness_path()),
        ] {
            expected.push(OsString::from(option));
            expected.push(path.as_os_str().to_os_string());
        }
        expected.extend([
            OsString::from("--admission-timeout-ms"),
            OsString::from("11"),
            OsString::from("--handshake-timeout-ms"),
            OsString::from("12"),
            OsString::from("--request-timeout-ms"),
            OsString::from("13"),
            OsString::from("--drain-timeout-ms"),
            OsString::from("14"),
            OsString::from("--admission-capacity"),
            OsString::from("3"),
            OsString::from("--requests-per-connection"),
            OsString::from("4"),
            OsString::from("--native-run-claim-lease-ms"),
            OsString::from("15"),
            OsString::from("--native-run-poll-interval-ms"),
            OsString::from("16"),
            OsString::from("--native-run-retry-backoff-ms"),
            OsString::from("17"),
            OsString::from("--native-run-shutdown-budget-ms"),
            OsString::from("18"),
            OsString::from("--native-run-queue-capacity"),
            OsString::from("5"),
            OsString::from("--native-run-max-command-retries"),
            OsString::from("6"),
            OsString::from("--native-run-prompt-delivery"),
            OsString::from("queue"),
            OsString::from("--native-run-stream-after"),
            OsString::from("7"),
        ]);
        assert_eq!(argv, expected);
    }

    #[test]
    fn foreground_and_background_commands_share_the_same_executable_and_argv() {
        let spec = test_launch_spec();
        let foreground = super::forge_command(&spec);
        let mut background = super::forge_command(&spec);
        background
            .stdin(super::Stdio::null())
            .stdout(super::Stdio::null())
            .stderr(super::Stdio::null());

        assert_eq!(foreground.get_program(), background.get_program());
        assert_eq!(
            foreground.get_args().collect::<Vec<_>>(),
            background.get_args().collect::<Vec<_>>()
        );
    }

    #[test]
    fn owned_processkit_command_projects_exact_launch_and_safe_policy() {
        let spec = test_launch_spec();
        let command = forge_owned_command_with_environment(
            &spec,
            [
                (OsString::from("PATH"), OsString::from("safe")),
                (OsString::from("ARTISAN_SECRET"), OsString::from("secret")),
                (OsString::from("NODE_OPTIONS"), OsString::from("legacy")),
                (OsString::from("ELECTRON_RUN_AS_NODE"), OsString::from("1")),
                (
                    OsString::from("CODEX_SQLITE_HOME"),
                    OsString::from("legacy.sqlite"),
                ),
            ],
        );

        assert_eq!(command.program(), spec.executable().as_os_str());
        assert_eq!(command.arguments(), spec.argv());
        assert!(command.stdin_source().is_some());
        let environment = command.env_overrides();
        assert!(environment.iter().any(|(key, value)| {
            key == OsStr::new("PATH") && value.as_deref() == Some(OsStr::new("safe"))
        }));
        assert!(
            environment
                .iter()
                .all(|(key, _)| !is_forbidden_environment_key(key))
        );

        // processkit's public Debug view exposes the non-secret launch intent
        // while redacting argv/environment values; use it only for the fields
        // that do not contain credential material.
        let debug = format!("{command:?}");
        assert!(!debug.contains("secret"));
        assert!(debug.contains("args: 40"));
        assert!(debug.contains("env_clear: true"));
        assert!(debug.contains("stdin: Some(Stdin(\"Empty\"))"));
        assert!(debug.contains("stdout_mode: Null"));
        assert!(debug.contains("stderr_mode: Null"));
        assert!(debug.contains("creation_flags_extra: 134217728"));
    }

    #[tokio::test]
    async fn forge_process_lease_debug_redacts_process_details() {
        let command = if cfg!(windows) {
            processkit::Command::new("cmd.exe").args(["/C", "ping", "-n", "6", "127.0.0.1"])
        } else {
            processkit::Command::new("sh").args(["-c", "sleep 5"])
        };
        let process = command
            .stdin(processkit::Stdin::empty())
            .stdout(processkit::StdioMode::Null)
            .stderr(processkit::StdioMode::Null)
            .create_no_window()
            .start()
            .await
            .expect("test process must start");
        let pid = process.pid().expect("test process must have a PID");
        let lease = ForgeProcessLease {
            process,
            pid,
            readiness: valid_readiness(pid),
        };

        let debug = format!("{lease:?}");
        assert!(debug.contains("ForgeProcessLease"));
        assert!(debug.contains(&format!("pid: {pid}")));
        assert!(!debug.contains("LEASE-CREDENTIAL-SECRET"));
        lease
            .shutdown(Duration::ZERO)
            .await
            .expect("test process teardown must be confirmed");
    }

    #[test]
    fn credential_manifest_mismatch_is_a_typed_launch_error() {
        let home = tempfile::tempdir().unwrap();
        let credentials = ForgeCredentialPaths::from_home(home.path()).unwrap();
        let config = test_native_config(home.path(), &home.path().join("other-manifest.json"));
        assert!(matches!(
            ForgeLaunchSpec::new(&test_manifest(), &config, &credentials),
            Err(CliError::CredentialManifestMismatch { .. })
        ));
    }

    #[test]
    fn native_forge_launch_drops_legacy_arguments_and_environment() {
        let spec = test_launch_spec();
        for forbidden in [
            "--listen-port",
            "--listen-host",
            "--host",
            "--mode",
            "--static-root",
            "--token",
            "--state",
            "--database-path",
            "--broker",
            "--node",
        ] {
            assert!(
                !spec
                    .argv
                    .iter()
                    .any(|argument| argument.as_os_str() == OsStr::new(forbidden))
            );
        }
        assert!(
            super::forge_command(&spec)
                .get_envs()
                .all(|(key, _)| !is_forbidden_environment_key(key))
        );

        let mut command = Command::new("forge");
        configure_environment(
            &mut command,
            [
                (OsString::from("PATH"), OsString::from("safe")),
                (OsString::from("ARTISAN_HOME"), OsString::from("legacy")),
                (
                    OsString::from("ARTISAN_AUTH_TOKEN"),
                    OsString::from("secret"),
                ),
                (
                    OsString::from("ARTISAN_DATABASE_PATH"),
                    OsString::from("legacy.db"),
                ),
                (
                    OsString::from("ARTISAN_FORGE_STATE_PATH"),
                    OsString::from("legacy.state"),
                ),
                (
                    OsString::from("ARTISAN_LISTEN_HOST"),
                    OsString::from("127.0.0.1"),
                ),
                (
                    OsString::from("ARTISAN_LISTEN_PORT"),
                    OsString::from("4317"),
                ),
                (
                    OsString::from("ARTISAN_FORGE_MODE"),
                    OsString::from("local"),
                ),
                (
                    OsString::from("ARTISAN_BROKER_PATH"),
                    OsString::from("legacy-broker"),
                ),
                (
                    OsString::from("ARTISAN_NODE_EXECUTABLE"),
                    OsString::from("legacy-node"),
                ),
                (
                    OsString::from("ARTISAN_STATIC_FRONTEND_ROOT"),
                    OsString::from("legacy.frontend"),
                ),
                (OsString::from("NODE_PATH"), OsString::from("legacy.node")),
                (OsString::from("ELECTRON_RUN_AS_NODE"), OsString::from("1")),
                (
                    OsString::from("CODEX_SQLITE_HOME"),
                    OsString::from("legacy.sqlite"),
                ),
            ],
        );
        assert!(
            command
                .get_envs()
                .any(|(key, value)| key == OsStr::new("PATH") && value == Some(OsStr::new("safe")))
        );
        assert!(
            command
                .get_envs()
                .all(|(key, _)| !is_forbidden_environment_key(key))
        );
    }

    fn valid_readiness(pid: u32) -> ForgeReadiness {
        ForgeReadiness::new(
            super::READY_SCHEMA,
            "127.0.0.1:4317",
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            pid,
        )
        .unwrap()
    }

    fn assert_spawn_preserves_prior(
        decision: BackgroundStartDecision,
        expected: &ReadinessFileSnapshot,
    ) {
        let BackgroundStartDecision::Spawn { prior_readiness } = decision else {
            panic!("existing receipt unexpectedly authorized AlreadyRunning");
        };
        let Some(prior) = prior_readiness else {
            panic!("existing receipt was not retained as a stale-file guard");
        };
        assert_eq!(&prior, expected);
        assert!(!readiness_file_replaced(Some(&prior), &prior));
    }

    #[test]
    fn live_existing_receipt_selects_already_running_without_spawning() {
        let expected = if cfg!(windows) {
            PathBuf::from(r"C:\Artisan\versions\1.2.3\bin\forge.exe")
        } else {
            PathBuf::from("/opt/Artisan/versions/1.2.3/bin/forge")
        };
        let actual = expected.clone();
        let existing = ReadinessFileRead::Present(ReadinessFileSnapshot {
            identity: Some(ReadinessFileIdentity {
                first: 1,
                second: 10,
            }),
            bytes: serde_json::to_vec(&valid_readiness(42)).unwrap(),
        });

        assert_eq!(
            background_start_decision(existing, &expected, move |_| Some(actual.clone())),
            BackgroundStartDecision::AlreadyRunning
        );
    }

    #[test]
    fn owned_launch_refuses_a_live_receipt_instead_of_adopting_it() {
        let expected = if cfg!(windows) {
            PathBuf::from(r"C:\Artisan\versions\1.2.3\bin\forge.exe")
        } else {
            PathBuf::from("/opt/Artisan/versions/1.2.3/bin/forge")
        };
        let actual = expected.clone();
        let existing = ReadinessFileRead::Present(ReadinessFileSnapshot {
            identity: Some(ReadinessFileIdentity {
                first: 1,
                second: 10,
            }),
            bytes: serde_json::to_vec(&valid_readiness(42)).unwrap(),
        });

        assert_eq!(
            owned_start_decision(existing, &expected, move |_| Some(actual)),
            OwnedStartDecision::RefuseAlreadyRunning
        );
        assert!(matches!(
            owned_already_running_failure(),
            CliError::Unsupported(message) if message == super::OWNED_FORGE_ALREADY_RUNNING
        ));
    }

    #[test]
    fn owned_process_identity_requires_an_exact_start_token() {
        let expected = 900;
        assert_eq!(
            owned_process_identity(expected, OwnedProcessStartInfo::Present(expected)),
            OwnedProcessIdentity::Match
        );
        for current in [
            OwnedProcessStartInfo::QueryFailed,
            OwnedProcessStartInfo::Missing,
            OwnedProcessStartInfo::MissingStartTime,
            OwnedProcessStartInfo::Present(expected + 1),
        ] {
            assert_eq!(
                owned_process_identity(expected, current),
                OwnedProcessIdentity::FailClosed
            );
            assert!(!owned_process_identity_matches(expected, current));
        }
    }

    #[test]
    fn stale_or_mismatched_existing_receipts_remain_spawn_only() {
        let expected = if cfg!(windows) {
            PathBuf::from(r"C:\Artisan\versions\1.2.3\bin\forge.exe")
        } else {
            PathBuf::from("/opt/Artisan/versions/1.2.3/bin/forge")
        };
        let identity = Some(ReadinessFileIdentity {
            first: 1,
            second: 10,
        });

        let stale = ReadinessFileSnapshot {
            identity,
            bytes: serde_json::to_vec(&valid_readiness(42)).unwrap(),
        };
        assert_spawn_preserves_prior(
            background_start_decision(ReadinessFileRead::Present(stale.clone()), &expected, |_| {
                None
            }),
            &stale,
        );

        let malformed = ReadinessFileSnapshot {
            identity,
            bytes: b"not-json".to_vec(),
        };
        assert_spawn_preserves_prior(
            background_start_decision(
                ReadinessFileRead::Present(malformed.clone()),
                &expected,
                |_| panic!("malformed receipt must not resolve a process"),
            ),
            &malformed,
        );

        let wrong_pid = ReadinessFileSnapshot {
            identity,
            bytes: serde_json::to_vec(&valid_readiness(7)).unwrap(),
        };
        let live_expected = expected.clone();
        assert_spawn_preserves_prior(
            background_start_decision(
                ReadinessFileRead::Present(wrong_pid.clone()),
                &expected,
                move |pid| {
                    if pid == 42 {
                        Some(live_expected.clone())
                    } else {
                        None
                    }
                },
            ),
            &wrong_pid,
        );

        let wrong_executable = ReadinessFileSnapshot {
            identity,
            bytes: serde_json::to_vec(&valid_readiness(42)).unwrap(),
        };
        let other = expected.with_file_name(if cfg!(windows) {
            "editor.exe"
        } else {
            "editor"
        });
        assert_spawn_preserves_prior(
            background_start_decision(
                ReadinessFileRead::Present(wrong_executable.clone()),
                &expected,
                move |_| Some(other.clone()),
            ),
            &wrong_executable,
        );

        assert_eq!(
            background_start_decision(ReadinessFileRead::Missing, &expected, |_| {
                panic!("missing receipt must not resolve a process")
            }),
            BackgroundStartDecision::Spawn {
                prior_readiness: None,
            }
        );
        assert_eq!(
            background_start_decision(ReadinessFileRead::Invalid, &expected, |_| {
                panic!("unsafe receipt must not resolve a process")
            }),
            BackgroundStartDecision::Spawn {
                prior_readiness: None,
            }
        );
    }

    #[test]
    fn readiness_receipt_round_trips_exactly_with_private_typed_fields() {
        let readiness = valid_readiness(42);
        let bytes = serde_json::to_vec(&readiness).unwrap();
        assert_eq!(
            String::from_utf8(bytes.clone()).unwrap(),
            r#"{"schema":"artisan-forge-ready-v1","endpoint":"127.0.0.1:4317","certificate_sha256":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","pid":42}"#
        );
        let decoded = ForgeReadiness::from_json(&bytes).unwrap();
        assert_eq!(decoded, readiness);
        assert_eq!(decoded.schema(), super::READY_SCHEMA);
        assert_eq!(decoded.endpoint(), "127.0.0.1:4317");
        assert_eq!(decoded.certificate_sha256().len(), 64);
        assert_eq!(decoded.pid(), 42);
    }

    #[test]
    fn readiness_rejects_every_frozen_validation_edge() {
        assert!(
            ForgeReadiness::new(
                "artisan-forge-ready-v2",
                "127.0.0.1:4317",
                "a".repeat(64),
                42,
            )
            .is_err()
        );

        for endpoint in [
            "127.0.0.1:0",
            "127.0.0.2:4317",
            "0.0.0.0:4317",
            "localhost:4317",
            "[::1]:4317",
            "http://127.0.0.1:4317",
            "127.0.0.1",
            "127.0.0.1:01",
            "127.0.0.1:65536",
        ] {
            assert!(
                ForgeReadiness::new(super::READY_SCHEMA, endpoint, "a".repeat(64), 42).is_err(),
                "endpoint {endpoint}"
            );
        }

        for hash in [
            String::new(),
            "a".to_owned(),
            "a".repeat(63),
            "a".repeat(65),
            "g".repeat(64),
            "é".repeat(64),
        ] {
            assert!(
                ForgeReadiness::new(super::READY_SCHEMA, "127.0.0.1:4317", hash.as_str(), 42)
                    .is_err(),
                "hash {hash:?}"
            );
        }

        assert!(
            ForgeReadiness::new(super::READY_SCHEMA, "127.0.0.1:4317", "A".repeat(64), 42,).is_ok()
        );
        assert!(
            ForgeReadiness::new(super::READY_SCHEMA, "127.0.0.1:4317", "a".repeat(64), 0,).is_err()
        );
        assert!(ForgeReadiness::from_json(
            br#"{"schema":"artisan-forge-ready-v1","endpoint":"127.0.0.1:4317","certificate_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","pid":42,"extra":true}"#
        )
        .is_err());
        let oversized = [b' '; super::MAX_READINESS_BYTES + 1];
        assert!(ForgeReadiness::from_json(&oversized).is_err());
        for pid in [0, u32::MAX] {
            let receipt = format!(
                r#"{{"schema":"artisan-forge-ready-v1","endpoint":"127.0.0.1:4317","certificate_sha256":"{}","pid":{pid}}}"#,
                "a".repeat(64)
            );
            if pid == 0 {
                assert!(ForgeReadiness::from_json(receipt.as_bytes()).is_err());
            } else {
                assert!(ForgeReadiness::from_json(receipt.as_bytes()).is_ok());
            }
        }
    }

    #[test]
    fn stale_receipts_require_a_replaced_file_identity() {
        let receipt = serde_json::to_vec(&valid_readiness(42)).unwrap();
        let prior = ReadinessFileSnapshot {
            identity: Some(ReadinessFileIdentity {
                first: 1,
                second: 10,
            }),
            bytes: receipt.clone(),
        };
        let unchanged = prior.clone();
        assert!(!readiness_file_replaced(Some(&prior), &unchanged));
        assert!(!readiness_file_replaced(
            Some(&prior),
            &ReadinessFileSnapshot {
                identity: prior.identity,
                bytes: serde_json::to_vec(&valid_readiness(7)).unwrap(),
            }
        ));
        assert!(readiness_file_replaced(
            Some(&prior),
            &ReadinessFileSnapshot {
                identity: Some(ReadinessFileIdentity {
                    first: 1,
                    second: 11,
                }),
                bytes: receipt,
            }
        ));
        assert!(readiness_file_replaced(None, &unchanged));
        assert!(!readiness_file_replaced(
            Some(&ReadinessFileSnapshot {
                identity: None,
                bytes: Vec::new(),
            }),
            &unchanged,
        ));
        assert!(!readiness_file_replaced(
            None,
            &ReadinessFileSnapshot {
                identity: None,
                bytes: Vec::new(),
            },
        ));
    }

    #[test]
    fn readiness_identity_rejects_wrong_pid_executable_and_pid_reuse() {
        let expected = if cfg!(windows) {
            PathBuf::from(r"C:\Artisan\versions\1.2.3\bin\forge.exe")
        } else {
            PathBuf::from("/opt/Artisan/versions/1.2.3/bin/forge")
        };
        let actual = expected.clone();
        let readiness = valid_readiness(42);
        assert!(readiness_matches_child(
            &readiness,
            42,
            &expected,
            |_| Some(actual.clone()),
        ));
        assert!(!readiness_matches_child(
            &readiness,
            7,
            &expected,
            |_| Some(actual.clone()),
        ));
        assert!(!readiness_matches_child(&readiness, 42, &expected, |_| {
            Some(expected.with_file_name(if cfg!(windows) {
                "editor.exe"
            } else {
                "editor"
            }))
        },));
        assert!(!readiness_matches_process(&readiness, &expected, |_| Some(
            PathBuf::from(if cfg!(windows) {
                r"C:\Windows\System32\notepad.exe"
            } else {
                "/usr/bin/notepad"
            })
        ),));
        assert!(!readiness_matches_child(&readiness, 42, &expected, |_| {
            None
        }));
    }

    struct FakeChild {
        pid: u32,
        termination: Option<ForgeTermination>,
    }

    impl ChildProbe for FakeChild {
        fn pid(&self) -> u32 {
            self.pid
        }

        fn poll_termination(&mut self) -> crate::Result<Option<ForgeTermination>> {
            Ok(self.termination.take())
        }
    }

    #[test]
    fn child_exit_before_readiness_preserves_each_known_forge_code() {
        for code in [64, 70, 71, 72, 73, 75] {
            let mut child = FakeChild {
                pid: 42,
                termination: Some(ForgeTermination::Exited(ForgeExitCode::from_code(code))),
            };
            let result = wait_for_readiness_with(
                &mut child,
                Path::new("/opt/Artisan/versions/1.2.3/bin/forge"),
                Path::new("/tmp/forge-ready.json"),
                None,
                Instant::now() + Duration::from_secs(30),
                |_| panic!("readiness must not be read after child exit"),
                |_| panic!("identity must not be queried after child exit"),
                |_, _| panic!("poll must not sleep after child exit"),
            );
            assert!(matches!(
                result,
                Err(CliError::ForgeTerminated {
                    termination: ForgeTermination::Exited(exit)
                }) if exit.code() == code
            ));
        }
    }

    #[test]
    fn readiness_success_is_tied_to_the_spawned_child_and_exact_forge_path() {
        let expected = Path::new("/opt/Artisan/versions/1.2.3/bin/forge");
        let readiness = valid_readiness(42);
        let snapshot = ReadinessFileSnapshot {
            identity: Some(ReadinessFileIdentity {
                first: 1,
                second: 2,
            }),
            bytes: serde_json::to_vec(&readiness).unwrap(),
        };
        let mut child = FakeChild {
            pid: 42,
            termination: None,
        };
        let result = wait_for_readiness_with(
            &mut child,
            expected,
            Path::new("/tmp/forge-ready.json"),
            None,
            Instant::now() + Duration::from_secs(30),
            move |_| ReadinessFileRead::Present(snapshot.clone()),
            move |_| Some(expected.to_path_buf()),
            |_, _| panic!("ready child must not sleep"),
        )
        .unwrap();
        assert_eq!(result, StartResult::Spawned { pid: 42 });
    }

    #[test]
    fn owned_readiness_requires_spawned_pid_exact_path_and_replaced_identity() {
        let expected = Path::new("/opt/Artisan/versions/1.2.3/bin/forge");
        let prior = ReadinessFileSnapshot {
            identity: Some(ReadinessFileIdentity {
                first: 1,
                second: 10,
            }),
            bytes: serde_json::to_vec(&valid_readiness(7)).unwrap(),
        };
        let current = ReadinessFileSnapshot {
            identity: Some(ReadinessFileIdentity {
                first: 1,
                second: 11,
            }),
            bytes: serde_json::to_vec(&valid_readiness(42)).unwrap(),
        };

        assert!(
            owned_readiness_candidate(
                ReadinessFileRead::Present(current.clone()),
                Some(&prior),
                42,
                expected,
                900,
                OwnedProcessStartInfo::Present(900),
                |_| Some(expected.to_path_buf()),
            )
            .is_some()
        );

        for unavailable in [
            OwnedProcessStartInfo::QueryFailed,
            OwnedProcessStartInfo::Missing,
            OwnedProcessStartInfo::MissingStartTime,
            OwnedProcessStartInfo::Present(901),
        ] {
            assert!(
                owned_readiness_candidate(
                    ReadinessFileRead::Present(current.clone()),
                    Some(&prior),
                    42,
                    expected,
                    900,
                    unavailable,
                    |_| Some(expected.to_path_buf()),
                )
                .is_none()
            );
        }

        let unchanged = ReadinessFileSnapshot {
            identity: prior.identity,
            ..current.clone()
        };
        assert!(
            owned_readiness_candidate(
                ReadinessFileRead::Present(unchanged),
                Some(&prior),
                42,
                expected,
                900,
                OwnedProcessStartInfo::Present(900),
                |_| Some(expected.to_path_buf()),
            )
            .is_none()
        );

        assert!(
            owned_readiness_candidate(
                ReadinessFileRead::Present(current.clone()),
                Some(&prior),
                7,
                expected,
                900,
                OwnedProcessStartInfo::Present(900),
                |_| Some(expected.to_path_buf()),
            )
            .is_none()
        );

        let other = expected.with_file_name("editor");
        assert!(
            owned_readiness_candidate(
                ReadinessFileRead::Present(current),
                Some(&prior),
                42,
                expected,
                900,
                OwnedProcessStartInfo::Present(900),
                |_| Some(other),
            )
            .is_none()
        );
    }

    #[test]
    fn readiness_poll_uses_one_absolute_deadline_and_bounded_interval() {
        let now = Instant::now();
        let deadline = now + Duration::from_secs(5);
        assert_eq!(
            poll_delay(deadline, Duration::from_millis(100), now),
            Some(Duration::from_millis(100))
        );
        assert_eq!(
            poll_delay(
                deadline,
                Duration::from_millis(100),
                now + Duration::from_secs(4) + Duration::from_millis(950)
            ),
            Some(Duration::from_millis(50))
        );
        assert_eq!(
            poll_delay(deadline, Duration::from_millis(100), deadline),
            None
        );
    }

    #[test]
    fn owned_shutdown_grace_is_clamped_to_two_seconds() {
        assert_eq!(clamp_shutdown_grace(Duration::ZERO), Duration::ZERO);
        assert_eq!(
            clamp_shutdown_grace(Duration::from_millis(1_500)),
            Duration::from_millis(1_500)
        );
        assert_eq!(
            clamp_shutdown_grace(FORGE_SHUTDOWN_GRACE_MAX),
            FORGE_SHUTDOWN_GRACE_MAX
        );
        assert_eq!(
            clamp_shutdown_grace(Duration::from_secs(3)),
            FORGE_SHUTDOWN_GRACE_MAX
        );
    }

    #[test]
    fn owned_shutdown_failure_is_a_bounded_io_error() {
        let error = owned_shutdown_failure();
        assert!(matches!(
            error,
            CliError::Io { context, source }
                if context == "shutdown Forge"
                    && source.to_string() == super::OWNED_FORGE_SHUTDOWN_FAILURE
        ));
    }

    #[test]
    fn owned_startup_outcomes_keep_existing_forge_termination_mapping() {
        assert!(matches!(
            forge_startup_outcome_error(processkit::Outcome::Exited(70)),
            CliError::ForgeTerminated { termination } if termination.exit_code() == Some(70)
        ));
        assert!(matches!(
            forge_startup_outcome_error(processkit::Outcome::Signalled(Some(9))),
            CliError::ForgeTerminated { termination } if termination.exit_code().is_none()
        ));
        assert!(matches!(
            forge_startup_outcome_error(processkit::Outcome::TimedOut),
            CliError::Control(message) if message == super::OWNED_FORGE_START_FAILURE
        ));
    }
}
