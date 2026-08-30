use std::{
    env,
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime},
};

use crate::{
    CliError, Result,
    credentials::ForgeCredentialPaths,
    error::io,
    http,
    instance::{InstancePaths, NativeInstanceConfig, Secrets, State},
    manifest::InstallationManifest,
};

const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(15);
const SHUTDOWN_PROBE_TIMEOUT: Duration = Duration::from_secs(1);
const SHUTDOWN_POLL_INTERVAL: Duration = Duration::from_millis(100);
const INSTANCE_REGISTRY_PROBE_TIMEOUT: Duration = Duration::from_millis(250);
const INSTANCE_REGISTRY_CARD_LIMIT: usize = 256;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ForgeLaunchSpec {
    executable: PathBuf,
    argv: Vec<OsString>,
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
        })
    }

    pub fn executable(&self) -> &Path {
        &self.executable
    }

    pub fn argv(&self) -> &[OsString] {
        &self.argv
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

fn native_argv(
    config: &NativeInstanceConfig,
    certificate_paths: &[PathBuf],
    private_key_path: &Path,
    capability_path: &Path,
) -> Vec<OsString> {
    let mut argv = Vec::with_capacity(22 + certificate_paths.len() * 2);
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

pub fn start(spec: &ForgeLaunchSpec, foreground: bool) -> Result<StartResult> {
    if foreground {
        start_foreground(spec)
    } else {
        spawn_background_forge(spec).map(|pid| StartResult::Spawned { pid })
    }
}

pub fn start_until(
    spec: &ForgeLaunchSpec,
    foreground: bool,
    _health_deadline: Instant,
) -> Result<StartResult> {
    start(spec, foreground)
}

fn start_foreground(spec: &ForgeLaunchSpec) -> Result<StartResult> {
    ensure_forge_executable(spec)?;
    let status = forge_command(spec).status().map_err(io("start Forge"))?;
    if status.success() {
        Ok(StartResult::ForegroundExited)
    } else {
        Err(CliError::Control(format!("Forge exited with {status}")))
    }
}

fn spawn_background_forge(spec: &ForgeLaunchSpec) -> Result<u32> {
    ensure_forge_executable(spec)?;
    let mut command = forge_command(spec);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    detach(&mut command);
    Ok(command.spawn().map_err(io("start Forge"))?.id())
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

pub fn stop(paths: &InstancePaths, secrets: &Secrets) -> Result<()> {
    stop_with_instance_id(paths, secrets, None)
}

/// Stops Forge only when the state still belongs to `expected_instance_id`.
/// An editor-owned cleanup must never shut down a replacement Forge, so a
/// missing or changed state is intentionally a successful no-op.
pub fn stop_with_instance_id(
    paths: &InstancePaths,
    secrets: &Secrets,
    expected_instance_id: Option<&str>,
) -> Result<()> {
    let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
    let Some(state) =
        live_state_selected_until(paths, secrets, expected_instance_id, None, deadline)?
    else {
        return match expected_instance_id {
            Some(_) => Ok(()),
            None => Err(CliError::NotRunning),
        };
    };
    stop_state(&state, secrets, deadline)
}

/// Stops only the authenticated Forge whose process identity the installer
/// discovered. A bare stop could otherwise shut down a newer replacement
/// while leaving the superseded process alive.
pub fn stop_with_pid(paths: &InstancePaths, secrets: &Secrets, expected_pid: u32) -> Result<()> {
    let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
    let Some(state) =
        live_state_selected_until(paths, secrets, None, Some(expected_pid), deadline)?
    else {
        return Err(CliError::NotRunning);
    };
    stop_state(&state, secrets, deadline)
}

/// Stops an installer-selected Forge only when its authenticated control
/// status proves that no model run would be interrupted.
pub fn stop_with_pid_if_idle(
    paths: &InstancePaths,
    secrets: &Secrets,
    expected_pid: u32,
) -> Result<()> {
    let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
    let selected = live_state_selected_until(paths, secrets, None, Some(expected_pid), deadline);
    let Some(state) = (match selected {
        Err(CliError::NotRunning) => return Err(CliError::ForgeActivityUnavailable),
        Err(error) => return Err(error),
        Ok(state) => state,
    }) else {
        return Err(CliError::ForgeActivityUnavailable);
    };
    let status = http::status_until(
        &state.endpoint,
        &secrets.auth_token,
        probe_deadline(deadline, SHUTDOWN_PROBE_TIMEOUT),
    )?;
    if !state_matches_status(&state, &status) {
        return Err(CliError::ForgeActivityUnavailable);
    }
    ensure_idle_for_shutdown(&status)?;
    stop_state(&state, secrets, deadline)
}

fn ensure_idle_for_shutdown(status: &http::StatusResponse) -> Result<()> {
    match status.active_work_count {
        None => Err(CliError::ForgeActivityUnavailable),
        Some(0) => Ok(()),
        Some(active_work_count) => Err(CliError::ForgeBusy { active_work_count }),
    }
}

fn stop_state(state: &State, secrets: &Secrets, deadline: Instant) -> Result<()> {
    http::request_until(
        &state.endpoint,
        "/api/control/shutdown",
        &secrets.auth_token,
        "POST",
        deadline,
    )?;
    while Instant::now() < deadline {
        let status = http::status_until(
            &state.endpoint,
            &secrets.auth_token,
            probe_deadline(deadline, SHUTDOWN_PROBE_TIMEOUT),
        );
        if !status.is_ok_and(|status| state_matches_status(state, &status)) {
            return Ok(());
        }
        sleep_until(deadline, SHUTDOWN_POLL_INTERVAL);
    }
    Err(CliError::Control(
        "Forge accepted shutdown but remained reachable".into(),
    ))
}

/// Resolves this home's live Forge through its mutable state first, then the
/// machine registry card the Forge itself owns. The native `ae` binary is the
/// lifecycle boundary even when `state.json` was lost; a registry card cannot
/// repair the permanent Rust CLI that must start Forge on a clean machine.
pub fn live_state_until(
    paths: &InstancePaths,
    secrets: &Secrets,
    expected_instance_id: Option<&str>,
    deadline: Instant,
) -> Result<Option<State>> {
    live_state_selected_until(paths, secrets, expected_instance_id, None, deadline)
}

fn live_state_selected_until(
    paths: &InstancePaths,
    secrets: &Secrets,
    expected_instance_id: Option<&str>,
    expected_pid: Option<u32>,
    deadline: Instant,
) -> Result<Option<State>> {
    live_state_selected_bounded_until(
        paths,
        secrets,
        expected_instance_id,
        expected_pid,
        deadline,
        INSTANCE_REGISTRY_CARD_LIMIT,
    )
}

fn live_state_selected_bounded_until(
    paths: &InstancePaths,
    secrets: &Secrets,
    expected_instance_id: Option<&str>,
    expected_pid: Option<u32>,
    deadline: Instant,
    registry_card_limit: usize,
) -> Result<Option<State>> {
    if let Some(state) = primary_state(paths)?
        && should_stop_instance(expected_instance_id, &state.instance_id)
        && expected_pid.is_none_or(|pid| pid == state.pid)
        && state_is_live(&state, secrets, deadline)
    {
        return Ok(Some(state));
    }

    for state in registered_states_bounded(
        paths,
        expected_instance_id,
        expected_pid,
        registry_card_limit,
    )? {
        if state_is_live(&state, secrets, deadline) {
            return Ok(Some(state));
        }
        if Instant::now() >= deadline {
            break;
        }
    }
    Ok(None)
}

fn primary_state(paths: &InstancePaths) -> Result<Option<State>> {
    let metadata = match fs::symlink_metadata(&paths.state) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(CliError::Io {
                context: "inspect Forge state",
                source,
            });
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(CliError::UnsafePath(paths.state.clone()));
    }
    Ok(crate::instance::read_json::<State>(&paths.state).ok())
}

/// Registry cards are only discovery hints. PID snapshots are both slow and
/// vulnerable to PID reuse; the authenticated status probe below is the sole
/// authority for whether a card still names its exact Forge process.
#[cfg(test)]
fn registered_states(
    paths: &InstancePaths,
    expected_instance_id: Option<&str>,
    expected_pid: Option<u32>,
) -> Result<Vec<State>> {
    registered_states_bounded(
        paths,
        expected_instance_id,
        expected_pid,
        INSTANCE_REGISTRY_CARD_LIMIT,
    )
}

fn registered_states_bounded(
    paths: &InstancePaths,
    expected_instance_id: Option<&str>,
    expected_pid: Option<u32>,
    card_limit: usize,
) -> Result<Vec<State>> {
    let root = paths
        .state
        .parent()
        .ok_or_else(|| CliError::UnsafePath(paths.state.clone()))?;
    let registry = root.join("instances");
    let metadata = match fs::symlink_metadata(&registry) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(CliError::Io {
                context: "inspect Forge instance registry",
                source,
            });
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(CliError::UnsafePath(registry));
    }
    if let Some(instance_id) = expected_instance_id {
        if !safe_instance_id(instance_id) {
            return Ok(Vec::new());
        }
        return Ok(
            read_registered_state(&registry.join(format!("{instance_id}.json")))
                .filter(|state| {
                    state.instance_id == instance_id
                        && expected_pid.is_none_or(|pid| pid == state.pid)
                })
                .into_iter()
                .collect(),
        );
    }

    let mut cards = fs::read_dir(&registry)
        .map_err(|source| CliError::Io {
            context: "enumerate Forge instance registry",
            source,
        })?
        .filter_map(std::result::Result::ok)
        .filter_map(|entry| {
            let file_type = entry.file_type().ok()?;
            if file_type.is_symlink() || !file_type.is_file() {
                return None;
            }
            let path = entry.path();
            if path.extension().is_none_or(|extension| extension != "json") {
                return None;
            }
            let modified = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            Some((modified, path))
        })
        .collect::<Vec<_>>();
    cards.sort_unstable_by_key(|entry| std::cmp::Reverse(entry.0));
    cards.truncate(card_limit.min(INSTANCE_REGISTRY_CARD_LIMIT));

    Ok(cards
        .into_iter()
        .filter_map(|(_, path)| {
            let file_name = path.file_stem()?.to_str()?;
            let state = read_registered_state(&path)?;
            (state.instance_id == file_name && expected_pid.is_none_or(|pid| pid == state.pid))
                .then_some(state)
        })
        .collect())
}

fn read_registered_state(path: &Path) -> Option<State> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return None;
    }
    crate::instance::read_json(path).ok()
}

fn state_is_live(state: &State, secrets: &Secrets, deadline: Instant) -> bool {
    http::status_until(
        &state.endpoint,
        &secrets.auth_token,
        probe_deadline(deadline, INSTANCE_REGISTRY_PROBE_TIMEOUT),
    )
    .is_ok_and(|status| state_matches_status(state, &status))
}

fn state_matches_status(state: &State, status: &http::StatusResponse) -> bool {
    status.instance_id == state.instance_id && status.pid == state.pid
}

fn safe_instance_id(instance_id: &str) -> bool {
    !instance_id.is_empty()
        && instance_id.len() <= 128
        && instance_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn probe_deadline(deadline: Instant, probe_timeout: Duration) -> Instant {
    deadline.min(Instant::now() + probe_timeout)
}

fn sleep_until(deadline: Instant, interval: Duration) {
    if let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        thread::sleep(interval.min(remaining));
    }
}

fn should_stop_instance(expected_instance_id: Option<&str>, actual_instance_id: &str) -> bool {
    expected_instance_id.is_none_or(|expected| expected == actual_instance_id)
}

#[cfg(test)]
mod tests {
    use std::{
        ffi::{OsStr, OsString},
        fs,
        io::{Read, Write},
        net::TcpListener,
        num::NonZeroU32,
        path::{Path, PathBuf},
        thread,
        time::{Duration, Instant},
    };

    use super::{
        Command, ForgeLaunchSpec, SHUTDOWN_POLL_INTERVAL, SHUTDOWN_PROBE_TIMEOUT, SHUTDOWN_TIMEOUT,
        configure_environment, ensure_idle_for_shutdown, is_forbidden_environment_key,
        live_state_selected_until, native_argv, registered_states, should_stop_instance,
    };

    use crate::{
        CliError,
        credentials::ForgeCredentialPaths,
        http::StatusResponse,
        instance::{InstancePaths, NativeInstanceConfig, NativeListenerConfig, Secrets},
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
        )
        .unwrap()
    }

    fn test_manifest() -> InstallationManifest {
        InstallationManifest {
            activation_state: "active".into(),
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
        assert_eq!(spec.argv.len(), 24);
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

    #[test]
    fn exact_stop_decision_is_a_noop_for_a_replaced_instance() {
        assert!(should_stop_instance(None, "current"));
        assert!(should_stop_instance(Some("current"), "current"));
        assert!(!should_stop_instance(Some("editor-owned"), "replacement"));
    }

    #[test]
    fn legacy_status_cannot_authorize_an_idle_only_shutdown() {
        let status = StatusResponse {
            active_work_count: None,
            instance_id: "forge-1".into(),
            pid: 6172,
        };

        assert!(matches!(
            ensure_idle_for_shutdown(&status),
            Err(CliError::ForgeActivityUnavailable)
        ));
    }

    #[test]
    fn busy_status_uses_the_safe_shutdown_refusal_exit_code() {
        let status = StatusResponse {
            active_work_count: Some(2),
            instance_id: "forge-1".into(),
            pid: 6172,
        };

        assert!(matches!(
            ensure_idle_for_shutdown(&status),
            Err(CliError::ForgeBusy {
                active_work_count: 2
            })
        ));
        assert!(
            ensure_idle_for_shutdown(&StatusResponse {
                active_work_count: Some(0),
                instance_id: "forge-1".into(),
                pid: 6172,
            })
            .is_ok()
        );
        assert_eq!(
            CliError::ForgeBusy {
                active_work_count: 1
            }
            .exit_code(),
            5
        );
        assert_eq!(CliError::ForgeActivityUnavailable.exit_code(), 6);
    }

    #[test]
    fn shutdown_has_one_global_budget_below_the_desktop_timeout() {
        assert!(SHUTDOWN_TIMEOUT <= Duration::from_secs(20));
        assert!(SHUTDOWN_PROBE_TIMEOUT < SHUTDOWN_TIMEOUT);
        assert!(SHUTDOWN_POLL_INTERVAL < SHUTDOWN_TIMEOUT);
    }

    #[test]
    fn registry_cards_are_bounded_named_and_defer_liveness_to_authenticated_probes() {
        let live_pid = std::process::id();
        let root = std::env::temp_dir().join(format!(
            "artisan-cli-registry-test-{}-{}",
            std::process::id(),
            line!()
        ));
        let registry = root.join("instances");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&registry).unwrap();
        fs::write(
            registry.join("forge_valid.json"),
            format!(
                r#"{{"endpoint":"http://127.0.0.1:4848/","instance_id":"forge_valid","pid":{live_pid}}}"#
            ),
        )
        .unwrap();
        fs::write(
            registry.join("forge_wrong_name.json"),
            format!(
                r#"{{"endpoint":"http://127.0.0.1:4849/","instance_id":"forge_other","pid":{live_pid}}}"#
            ),
        )
        .unwrap();
        fs::write(
            registry.join("forge_dead.json"),
            br#"{"endpoint":"http://127.0.0.1:4850/","instance_id":"forge_dead","pid":2147483647}"#,
        )
        .unwrap();
        fs::write(registry.join("not-json.txt"), b"ignored").unwrap();
        let paths = InstancePaths {
            config: root.join("config.json"),
            secrets: root.join("secrets.json"),
            state: root.join("state.json"),
            log: root.join("forge.log"),
        };

        let states = registered_states(&paths, None, None).unwrap();
        let instance_ids = states
            .iter()
            .map(|state| state.instance_id.as_str())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(
            instance_ids,
            std::collections::HashSet::from(["forge_valid", "forge_dead"])
        );
        assert_eq!(
            registered_states(&paths, Some("forge_valid"), Some(live_pid))
                .unwrap()
                .len(),
            1
        );
        assert!(
            registered_states(&paths, Some("../escape"), None)
                .unwrap()
                .is_empty()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_primary_state_recovers_only_an_authenticated_exact_pid_card() {
        let live_pid = std::process::id();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut connection, _) = listener.accept().unwrap();
            let mut request = [0_u8; 2048];
            let read = connection.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..read]);
            assert!(request.contains("GET /api/control/status"));
            assert!(request.contains("Authorization: Bearer registry-token"));
            let body = format!(r#"{{"instance_id":"forge_recovered","pid":{live_pid}}}"#);
            connection
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .unwrap();
        });
        let root = tempfile::tempdir().unwrap();
        let registry = root.path().join("instances");
        fs::create_dir(&registry).unwrap();
        fs::write(
            registry.join("forge_recovered.json"),
            format!(
                r#"{{"endpoint":"http://127.0.0.1:{port}/","instance_id":"forge_recovered","pid":{live_pid}}}"#
            ),
        )
        .unwrap();
        let paths = InstancePaths {
            config: root.path().join("config.json"),
            secrets: root.path().join("secrets.json"),
            state: root.path().join("state.json"),
            log: root.path().join("forge.log"),
        };
        let secrets = Secrets {
            auth_token: "registry-token".into(),
            version: 1,
        };

        let state = live_state_selected_until(
            &paths,
            &secrets,
            None,
            Some(live_pid),
            Instant::now() + Duration::from_secs(2),
        )
        .unwrap()
        .expect("authenticated registry state");
        assert_eq!(state.instance_id, "forge_recovered");
        assert_eq!(state.pid, live_pid);
        server.join().unwrap();
    }
}
