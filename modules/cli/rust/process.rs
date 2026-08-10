use std::{
    fs::{self, OpenOptions},
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use crate::{
    CliError, Result,
    error::io,
    http,
    instance::{InstanceConfig, InstancePaths, Secrets, State},
    manifest::InstallationManifest,
};

const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(15);
const SHUTDOWN_PROBE_TIMEOUT: Duration = Duration::from_secs(1);
const SHUTDOWN_POLL_INTERVAL: Duration = Duration::from_millis(100);

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
        Instant::now() + Duration::from_secs(5),
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
    if let Ok(state) = crate::instance::read_json::<State>(&paths.state)
        && http::status_until(&state.endpoint, &secrets.auth_token, health_deadline)
            .is_ok_and(|status| status.instance_id == state.instance_id && status.pid == state.pid)
    {
        return Ok(StartResult::AlreadyRunning);
    }
    if !foreground {
        ensure_background_start_deadline(health_deadline)?;
    }
    let executable = manifest.forge_executable();
    let forge_root = manifest.version_root().join("forge");
    let legacy_host_entry = forge_root.join("host.js");
    if !executable.is_file() {
        return Err(CliError::Installation(format!(
            "Forge binary is missing at {}",
            executable.display()
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
    if foreground {
        let status = command.status().map_err(io("start Forge"))?;
        if status.success() {
            Ok(StartResult::ForegroundExited)
        } else {
            Err(CliError::Control(format!("Forge exited with {status}")))
        }
    } else {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone().map_err(io("clone Forge log"))?))
            .stderr(Stdio::from(log));
        detach(&mut command);
        ensure_background_start_deadline(health_deadline)?;
        let child = command.spawn().map_err(io("start Forge"))?;
        Ok(StartResult::Spawned { pid: child.id() })
    }
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
    let state_metadata = match fs::symlink_metadata(&paths.state) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return match expected_instance_id {
                Some(_) => Ok(()),
                None => Err(CliError::NotRunning),
            };
        }
        Err(source) => {
            return Err(CliError::Io {
                context: "inspect Forge state",
                source,
            });
        }
    };
    if state_metadata.file_type().is_symlink() || !state_metadata.is_file() {
        return Err(CliError::UnsafePath(paths.state.clone()));
    }
    let state: State = crate::instance::read_json(&paths.state)?;
    if !should_stop_instance(expected_instance_id, &state.instance_id) {
        return Ok(());
    }
    if let Some(expected_instance_id) = expected_instance_id {
        let Ok(status) = http::status_until(
            &state.endpoint,
            &secrets.auth_token,
            probe_deadline(deadline, SHUTDOWN_PROBE_TIMEOUT),
        ) else {
            // The process may have exited or a replacement may now own this
            // endpoint. Exact cleanup treats either race as a no-op.
            return Ok(());
        };
        if status.instance_id != expected_instance_id {
            return Ok(());
        }
        if crate::instance::read_json::<State>(&paths.state)
            .map_or(true, |current| current.instance_id != expected_instance_id)
        {
            return Ok(());
        }
    }
    http::request_until(
        &state.endpoint,
        "/api/control/shutdown",
        &secrets.auth_token,
        "POST",
        deadline,
    )?;
    while Instant::now() < deadline {
        if http::status_until(
            &state.endpoint,
            &secrets.auth_token,
            probe_deadline(deadline, SHUTDOWN_PROBE_TIMEOUT),
        )
        .is_err()
        {
            return Ok(());
        }
        sleep_until(deadline, SHUTDOWN_POLL_INTERVAL);
    }
    Err(CliError::Control(
        "Forge accepted shutdown but remained reachable".into(),
    ))
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
        path::{Path, PathBuf},
        time::{Duration, Instant},
    };

    use super::{
        Command, InstanceConfig, InstancePaths, SHUTDOWN_POLL_INTERVAL, SHUTDOWN_PROBE_TIMEOUT,
        SHUTDOWN_TIMEOUT, Secrets, background_start_can_continue, configure_forge_environment,
        should_stop_instance,
    };
    use crate::instance::ForgeMode;

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
    fn shutdown_has_one_global_budget_below_the_desktop_timeout() {
        assert!(SHUTDOWN_TIMEOUT <= Duration::from_secs(20));
        assert!(SHUTDOWN_PROBE_TIMEOUT < SHUTDOWN_TIMEOUT);
        assert!(SHUTDOWN_POLL_INTERVAL < SHUTDOWN_TIMEOUT);
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
}
