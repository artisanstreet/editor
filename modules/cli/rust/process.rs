use std::{
    fs::{self, OpenOptions},
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime},
};

use fs2::FileExt;

use crate::{
    CliError, Result,
    error::io,
    http,
    instance::{ForgeMode, InstanceConfig, InstancePaths, Secrets, State},
    manifest::InstallationManifest,
};

const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(15);
const SHUTDOWN_PROBE_TIMEOUT: Duration = Duration::from_secs(1);
const SHUTDOWN_POLL_INTERVAL: Duration = Duration::from_millis(100);
const INSTANCE_REGISTRY_PROBE_TIMEOUT: Duration = Duration::from_millis(250);
const INSTANCE_REGISTRY_CARD_LIMIT: usize = 256;
const PRELAUNCH_DISCOVERY_TIMEOUT: Duration = Duration::from_millis(250);
const PRELAUNCH_REGISTRY_CARD_LIMIT: usize = 1;
const START_COORDINATION_POLL_INTERVAL: Duration = Duration::from_millis(10);
const START_READY_POLL_INTERVAL: Duration = Duration::from_millis(100);
// Keep `ae start` aligned with `ae open --handoff`: a cold Forge may need up
// to 30 seconds before its authenticated state becomes ready.
const START_READY_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StartResult {
    AlreadyRunning,
    Spawned { pid: u32 },
    ForegroundExited,
}

pub fn start(
    manifest: &InstallationManifest,
    paths: &InstancePaths,
    config: &InstanceConfig,
    secrets: &Secrets,
    foreground: bool,
) -> Result<StartResult> {
    start_until(
        manifest,
        paths,
        config,
        secrets,
        foreground,
        Instant::now() + START_READY_TIMEOUT,
    )
}

pub fn start_until(
    manifest: &InstallationManifest,
    paths: &InstancePaths,
    config: &InstanceConfig,
    secrets: &Secrets,
    foreground: bool,
    health_deadline: Instant,
) -> Result<StartResult> {
    if foreground {
        let discovery_deadline = prelaunch_discovery_deadline(Instant::now(), health_deadline);
        if prelaunch_live_state_until(paths, secrets, discovery_deadline)?.is_some() {
            return Ok(StartResult::AlreadyRunning);
        }
        return start_foreground(manifest, paths, config, secrets);
    }

    with_start_coordination(paths, health_deadline, || {
        // Every background caller decides liveness while holding the home-local
        // launch lease. A concurrent caller therefore re-probes after the first
        // launch becomes ready instead of authorizing a second process.
        let discovery_deadline = prelaunch_discovery_deadline(Instant::now(), health_deadline);
        if prelaunch_live_state_until(paths, secrets, discovery_deadline)?.is_some() {
            return Ok(StartResult::AlreadyRunning);
        }
        ensure_background_start_deadline(health_deadline)?;
        let pid = spawn_background_forge(manifest, paths, config, secrets)?;
        wait_until_ready(paths, secrets, health_deadline)?;
        Ok(StartResult::Spawned { pid })
    })
}

fn start_foreground(
    manifest: &InstallationManifest,
    paths: &InstancePaths,
    config: &InstanceConfig,
    secrets: &Secrets,
) -> Result<StartResult> {
    let executable = manifest.forge_executable();
    let broker = manifest.broker_executable();
    let forge_root = manifest.version_root().join("forge");
    let legacy_host_entry = forge_root.join("host.js");
    if !executable.is_file() {
        return Err(CliError::Installation(format!(
            "Forge binary is missing at {}",
            executable.display()
        )));
    }
    if !broker.is_file() {
        return Err(CliError::Installation(format!(
            "Artisan Broker is missing at {}",
            broker.display()
        )));
    }
    fs::create_dir_all(&config.data_root).map_err(io("create Forge data directory"))?;
    let mut command = Command::new(executable);
    let legacy_launcher = legacy_host_entry.is_file();
    if legacy_launcher {
        command.arg(&legacy_host_entry);
    }
    configure_forge_environment(
        &mut command,
        paths,
        config,
        secrets,
        &forge_root,
        legacy_launcher,
    );
    configure_broker_environment(&mut command, &broker);
    let status = command.status().map_err(io("start Forge"))?;
    if status.success() {
        Ok(StartResult::ForegroundExited)
    } else {
        Err(CliError::Control(format!("Forge exited with {status}")))
    }
}

fn spawn_background_forge(
    manifest: &InstallationManifest,
    paths: &InstancePaths,
    config: &InstanceConfig,
    secrets: &Secrets,
) -> Result<u32> {
    let executable = manifest.forge_executable();
    let broker = manifest.broker_executable();
    let forge_root = manifest.version_root().join("forge");
    let legacy_host_entry = forge_root.join("host.js");
    if !executable.is_file() {
        return Err(CliError::Installation(format!(
            "Forge binary is missing at {}",
            executable.display()
        )));
    }
    if !broker.is_file() {
        return Err(CliError::Installation(format!(
            "Artisan Broker is missing at {}",
            broker.display()
        )));
    }
    fs::create_dir_all(&config.data_root).map_err(io("create Forge data directory"))?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.log)
        .map_err(io("open Forge log"))?;
    let mut command = Command::new(executable);
    let legacy_launcher = legacy_host_entry.is_file();
    if legacy_launcher {
        command.arg(&legacy_host_entry);
    }
    configure_forge_environment(
        &mut command,
        paths,
        config,
        secrets,
        &forge_root,
        legacy_launcher,
    );
    configure_broker_environment(&mut command, &broker);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone().map_err(io("clone Forge log"))?))
        .stderr(Stdio::from(log));
    detach(&mut command);
    Ok(command.spawn().map_err(io("start Forge"))?.id())
}

fn wait_until_ready(paths: &InstancePaths, secrets: &Secrets, deadline: Instant) -> Result<()> {
    while Instant::now() < deadline {
        if live_state_until(
            paths,
            secrets,
            None,
            probe_deadline(deadline, INSTANCE_REGISTRY_PROBE_TIMEOUT),
        )?
        .is_some()
        {
            return Ok(());
        }
        sleep_until(deadline, START_READY_POLL_INTERVAL);
    }
    Err(CliError::Control("Forge did not become ready".into()))
}

fn with_start_coordination<T>(
    paths: &InstancePaths,
    deadline: Instant,
    operation: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let root = paths
        .state
        .parent()
        .ok_or_else(|| CliError::UnsafePath(paths.state.clone()))?;
    let lock_path = root.join(".forge-start.lock");
    match fs::symlink_metadata(&lock_path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(CliError::UnsafePath(lock_path));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(CliError::Io {
                context: "inspect Forge start lock",
                source,
            });
        }
    }
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let lock = options
        .open(&lock_path)
        .map_err(io("open Forge start lock"))?;
    loop {
        match FileExt::try_lock_exclusive(&lock) {
            Ok(()) => break,
            Err(error) if start_lock_is_contended(&error) && Instant::now() < deadline => {
                sleep_until(deadline, START_COORDINATION_POLL_INTERVAL);
            }
            Err(error) if start_lock_is_contended(&error) => {
                return Err(CliError::Control(
                    "Forge start coordination timed out".into(),
                ));
            }
            Err(source) => {
                return Err(CliError::Io {
                    context: "lock Forge start coordination",
                    source,
                });
            }
        }
    }
    operation()
}

fn start_lock_is_contended(error: &std::io::Error) -> bool {
    if error.kind() == std::io::ErrorKind::WouldBlock {
        return true;
    }
    #[cfg(target_os = "windows")]
    {
        // fs2 preserves LockFileEx's ERROR_LOCK_VIOLATION instead of mapping it
        // to WouldBlock on Windows.
        error.raw_os_error() == Some(33)
    }
    #[cfg(not(target_os = "windows"))]
    false
}

fn ensure_background_start_deadline(deadline: Instant) -> Result<()> {
    if background_start_can_continue(Instant::now(), deadline) {
        Ok(())
    } else {
        Err(CliError::Control(
            "Forge start timed out before background launch".into(),
        ))
    }
}

fn background_start_can_continue(now: Instant, deadline: Instant) -> bool {
    now < deadline
}

fn prelaunch_discovery_deadline(now: Instant, health_deadline: Instant) -> Instant {
    health_deadline.min(now + PRELAUNCH_DISCOVERY_TIMEOUT)
}

fn configure_forge_environment(
    command: &mut Command,
    paths: &InstancePaths,
    config: &InstanceConfig,
    secrets: &Secrets,
    forge_root: &Path,
    legacy_launcher: bool,
) {
    command
        .env("ARTISAN_AUTH_TOKEN", &secrets.auth_token)
        .env(
            "ARTISAN_DATABASE_PATH",
            config.data_root.join("artisan.sqlite"),
        )
        .env("CODEX_SQLITE_HOME", config.data_root.join("codex-sqlite"))
        .env("ARTISAN_FORGE_STATE_PATH", &paths.state)
        .env("ARTISAN_FORGE_LOG_PATH", &paths.log)
        .env(
            "ARTISAN_TELEMETRY_CONFIG_PATH",
            paths.config.with_file_name("telemetry.json"),
        )
        .env(
            "ARTISAN_FORGE_MODE",
            match config.mode {
                ForgeMode::Local => "local",
                ForgeMode::Headless => "headless",
            },
        )
        .env("ARTISAN_LISTEN_HOST", &config.listen_host)
        .env("ARTISAN_LISTEN_PORT", config.listen_port.to_string());
    if legacy_launcher {
        configure_legacy_node_launcher(command, forge_root);
    }
    // Web hosting is a development capability. Without the flag, Forge
    // exposes only its health and control/WS surfaces and SPA routes 404.
    if config.serve_frontend {
        command.env("ARTISAN_STATIC_FRONTEND_ROOT", forge_root.join("frontend"));
    }
}

fn configure_broker_environment(command: &mut Command, broker: &Path) {
    command
        .env("ARTISAN_BROKER_PATH", broker)
        .env("ARTISAN_BROKER_REQUIRED", "1");
}

/// Supports installations from before Forge became a self-contained Node SEA.
/// New release payloads deliberately omit this entire loose Node runtime shape.
fn configure_legacy_node_launcher(command: &mut Command, forge_root: &Path) {
    let native_runtime = forge_root.join("native-runtime");
    command
        .env("ARTISAN_MIGRATIONS_PATH", forge_root.join("migrations"))
        .env("ARTISAN_NODE_EXECUTABLE", forge_root.join(node_name()))
        .env(
            "ARTISAN_WINDOWS_PROCESS_HOST",
            forge_root.join("windows-process-host.js"),
        )
        .env("ARTISAN_NATIVE_RUNTIME", &native_runtime)
        .env("NODE_PATH", &native_runtime);
}

#[cfg(target_os = "windows")]
const fn node_name() -> &'static str {
    "node.exe"
}

#[cfg(not(target_os = "windows"))]
const fn node_name() -> &'static str {
    "node"
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
/// lifecycle boundary even when `state.json` was lost: Effect's filesystem and
/// process services live inside Forge's Node runtime and cannot repair the
/// permanent Rust CLI that must start it on a clean machine.
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

fn prelaunch_live_state_until(
    paths: &InstancePaths,
    secrets: &Secrets,
    deadline: Instant,
) -> Result<Option<State>> {
    live_state_selected_bounded_until(
        paths,
        secrets,
        None,
        None,
        deadline,
        PRELAUNCH_REGISTRY_CARD_LIMIT,
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
        ffi::OsStr,
        fs,
        io::{Read, Write},
        net::TcpListener,
        path::{Path, PathBuf},
        sync::{
            Arc, Barrier,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
        thread,
        time::{Duration, Instant},
    };

    use super::{
        Command, InstanceConfig, InstancePaths, SHUTDOWN_POLL_INTERVAL, SHUTDOWN_PROBE_TIMEOUT,
        SHUTDOWN_TIMEOUT, START_READY_POLL_INTERVAL, START_READY_TIMEOUT, Secrets,
        background_start_can_continue, configure_broker_environment, configure_forge_environment,
        ensure_idle_for_shutdown, live_state_selected_until, live_state_until,
        prelaunch_discovery_deadline, registered_states, should_stop_instance,
        with_start_coordination,
    };
    use crate::instance::ForgeMode;
    use crate::{CliError, http::StatusResponse};

    fn test_instance(serve_frontend: bool) -> (InstancePaths, InstanceConfig, Secrets) {
        let home = PathBuf::from("C:/artisan-home");
        (
            InstancePaths {
                config: home.join("config.json"),
                secrets: home.join("secrets.json"),
                state: home.join("state.json"),
                log: home.join("forge.log"),
            },
            InstanceConfig {
                data_root: home.join("data"),
                listen_host: "127.0.0.1".into(),
                listen_port: 0,
                mode: ForgeMode::Local,
                serve_frontend,
                version: 1,
            },
            Secrets {
                auth_token: "token".into(),
                version: 1,
            },
        )
    }

    /// Installed-home gate: without the explicit development flag, the
    /// launched Forge never receives a static frontend root, so it cannot
    /// host the web renderer.
    #[test]
    fn static_hosting_is_absent_unless_the_home_opts_in() {
        let forge_root = Path::new("C:/Artisan/versions/1.0.0/forge");
        let (paths, config, secrets) = test_instance(false);
        let mut command = Command::new("forge");
        configure_forge_environment(&mut command, &paths, &config, &secrets, forge_root, false);
        assert!(
            command
                .get_envs()
                .all(|(key, _)| key != OsStr::new("ARTISAN_STATIC_FRONTEND_ROOT"))
        );

        let (paths, config, secrets) = test_instance(true);
        let mut serving = Command::new("forge");
        configure_forge_environment(&mut serving, &paths, &config, &secrets, forge_root, false);
        assert!(serving.get_envs().any(|(key, value)| {
            key == OsStr::new("ARTISAN_STATIC_FRONTEND_ROOT")
                && value.is_some_and(|path| Path::new(path).ends_with("frontend"))
        }));
    }

    #[test]
    fn sea_launch_environment_has_no_loose_node_runtime_dependencies() {
        let mut command = Command::new("forge");
        let forge_root = Path::new("C:/Artisan/forge");
        let (paths, config, secrets) = test_instance(false);

        configure_forge_environment(&mut command, &paths, &config, &secrets, forge_root, false);

        let environment = command.get_envs().collect::<Vec<_>>();
        for name in [
            "ARTISAN_MIGRATIONS_PATH",
            "ARTISAN_NATIVE_RUNTIME",
            "ARTISAN_NODE_EXECUTABLE",
            "ARTISAN_WINDOWS_PROCESS_HOST",
            "NODE_PATH",
        ] {
            assert!(environment.iter().all(|(key, _)| *key != OsStr::new(name)));
        }
    }

    #[test]
    fn installed_launch_requires_the_versioned_broker() {
        let mut command = Command::new("forge");
        let broker = Path::new("C:/Artisan/forge/Artisan Broker.exe");
        configure_broker_environment(&mut command, broker);
        assert!(command.get_envs().any(|(key, value)| {
            key == OsStr::new("ARTISAN_BROKER_PATH")
                && value.is_some_and(|path| path == broker.as_os_str())
        }));
        assert!(command.get_envs().any(|(key, value)| {
            key == OsStr::new("ARTISAN_BROKER_REQUIRED")
                && value.is_some_and(|marker| marker == OsStr::new("1"))
        }));
    }

    #[test]
    fn legacy_host_installations_keep_their_node_launcher_environment() {
        let mut command = Command::new("forge");
        let forge_root = Path::new("C:/Artisan/forge");
        let native_runtime = forge_root.join("native-runtime");
        let (paths, config, secrets) = test_instance(false);

        configure_forge_environment(&mut command, &paths, &config, &secrets, forge_root, true);

        let environment = command.get_envs().collect::<Vec<_>>();
        for name in ["ARTISAN_NATIVE_RUNTIME", "NODE_PATH"] {
            assert!(environment.iter().any(|(key, value)| {
                *key == OsStr::new(name) && value.as_deref() == Some(native_runtime.as_os_str())
            }));
        }
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
    fn default_background_start_allows_the_installed_cold_start_budget() {
        assert_eq!(START_READY_TIMEOUT, Duration::from_secs(30));
        assert!(START_READY_POLL_INTERVAL < START_READY_TIMEOUT);
    }

    #[test]
    fn background_start_never_continues_at_or_after_its_deadline() {
        let now = Instant::now();
        assert!(background_start_can_continue(
            now,
            now + Duration::from_millis(1)
        ));
        assert!(!background_start_can_continue(now, now));
    }

    #[test]
    fn prelaunch_discovery_cannot_consume_the_readiness_budget() {
        let now = Instant::now();
        let readiness_deadline = now + Duration::from_secs(15);
        assert_eq!(
            prelaunch_discovery_deadline(now, readiness_deadline),
            now + Duration::from_millis(250)
        );
    }

    #[test]
    fn prelaunch_discovery_accepts_an_authenticated_status_slower_than_the_old_cutoff() {
        let live_pid = std::process::id();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut connection, _) = listener.accept().unwrap();
            let mut request = [0_u8; 2048];
            let read = connection.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..read]);
            assert!(request.contains("GET /api/control/status"));
            assert!(request.contains("Authorization: Bearer delayed-token"));
            thread::sleep(Duration::from_millis(150));
            let body = format!(r#"{{"instance_id":"forge_delayed","pid":{live_pid}}}"#);
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
        let paths = InstancePaths {
            config: root.path().join("config.json"),
            secrets: root.path().join("secrets.json"),
            state: root.path().join("state.json"),
            log: root.path().join("forge.log"),
        };
        fs::write(
            &paths.state,
            format!(
                r#"{{"endpoint":"http://127.0.0.1:{port}/","instance_id":"forge_delayed","pid":{live_pid}}}"#
            ),
        )
        .unwrap();
        let secrets = Secrets {
            auth_token: "delayed-token".into(),
            version: 1,
        };
        let now = Instant::now();

        let state = live_state_until(
            &paths,
            &secrets,
            None,
            prelaunch_discovery_deadline(now, now + Duration::from_secs(15)),
        )
        .unwrap()
        .expect("delayed authenticated state");

        assert_eq!(state.instance_id, "forge_delayed");
        server.join().unwrap();
    }

    #[test]
    fn start_coordination_serializes_reprobe_before_one_spawn() {
        let root = tempfile::tempdir().unwrap();
        let paths = InstancePaths {
            config: root.path().join("config.json"),
            secrets: root.path().join("secrets.json"),
            state: root.path().join("state.json"),
            log: root.path().join("forge.log"),
        };
        let ready = Arc::new(AtomicBool::new(false));
        let spawn_count = Arc::new(AtomicUsize::new(0));
        let start_gate = Arc::new(Barrier::new(3));
        let mut workers = Vec::new();

        for _ in 0..2 {
            let paths = paths.clone();
            let ready = Arc::clone(&ready);
            let spawn_count = Arc::clone(&spawn_count);
            let start_gate = Arc::clone(&start_gate);
            workers.push(thread::spawn(move || {
                start_gate.wait();
                with_start_coordination(&paths, Instant::now() + Duration::from_secs(2), || {
                    if !ready.load(Ordering::SeqCst) {
                        thread::sleep(Duration::from_millis(50));
                        spawn_count.fetch_add(1, Ordering::SeqCst);
                        ready.store(true, Ordering::SeqCst);
                    }
                    Ok(())
                })
                .unwrap();
            }));
        }
        start_gate.wait();
        for worker in workers {
            worker.join().unwrap();
        }

        assert_eq!(spawn_count.load(Ordering::SeqCst), 1);
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
