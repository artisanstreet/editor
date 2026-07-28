use std::{
    fs::{self, OpenOptions},
    path::Path,
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use crate::{
    CliError, Result,
    error::io,
    http,
    instance::{InstanceConfig, InstancePaths, Secrets, State},
    manifest::InstallationManifest,
};

pub fn start(
    manifest: &InstallationManifest,
    paths: &InstancePaths,
    config: &InstanceConfig,
    secrets: &Secrets,
    foreground: bool,
) -> Result<()> {
    if let Ok(state) = crate::instance::read_json::<State>(&paths.state)
        && http::healthy(&state.endpoint, &secrets.auth_token)
    {
        return Ok(());
    }
    let executable = manifest.forge_executable();
    let forge_root = manifest.version_root().join("forge");
    let native_runtime = forge_root.join("native-runtime");
    let host_entry = forge_root.join("host.js");
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
    if host_entry.is_file() {
        command.arg(&host_entry);
    }
    configure_forge_environment(&mut command, paths, config, secrets, &forge_root);
    configure_native_runtime(&mut command, &native_runtime);
    if foreground {
        let status = command.status().map_err(io("start Forge"))?;
        if !status.success() {
            return Err(CliError::Control(format!("Forge exited with {status}")));
        }
    } else {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone().map_err(io("clone Forge log"))?))
            .stderr(Stdio::from(log));
        detach(&mut command);
        command.spawn().map_err(io("start Forge"))?;
    }
    Ok(())
}

fn configure_forge_environment(
    command: &mut Command,
    paths: &InstancePaths,
    config: &InstanceConfig,
    secrets: &Secrets,
    forge_root: &Path,
) {
    command
        .env("ARTISAN_AUTH_TOKEN", &secrets.auth_token)
        .env(
            "ARTISAN_DATABASE_PATH",
            config.data_root.join("artisan.sqlite"),
        )
        .env("ARTISAN_MIGRATIONS_PATH", forge_root.join("migrations"))
        .env("ARTISAN_NODE_EXECUTABLE", forge_root.join(node_name()))
        .env(
            "ARTISAN_WINDOWS_PROCESS_HOST",
            forge_root.join("windows-process-host.js"),
        )
        .env("CODEX_SQLITE_HOME", config.data_root.join("codex-sqlite"))
        .env("ARTISAN_FORGE_STATE_PATH", &paths.state)
        .env("ARTISAN_FORGE_LOG_PATH", &paths.log)
        .env("ARTISAN_LISTEN_HOST", &config.listen_host)
        .env("ARTISAN_LISTEN_PORT", config.listen_port.to_string());
    // Web hosting is a development capability. Without the flag, Forge
    // exposes only its health and control/WS surfaces and SPA routes 404.
    if config.serve_frontend {
        command.env("ARTISAN_STATIC_FRONTEND_ROOT", forge_root.join("frontend"));
    }
}

fn configure_native_runtime(command: &mut Command, native_runtime: &Path) {
    command
        .env("ARTISAN_NATIVE_RUNTIME", native_runtime)
        .env("NODE_PATH", native_runtime);
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
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    command.creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS);
}

#[cfg(not(target_os = "windows"))]
fn detach(_: &mut Command) {
    // std has no portable daemon/session API. Redirected stdio still makes the
    // child independent of this terminal; installers may add a service manager.
}

pub fn stop(paths: &InstancePaths, secrets: &Secrets) -> Result<()> {
    let state_metadata = match fs::symlink_metadata(&paths.state) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(CliError::NotRunning);
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
    http::request(
        &state.endpoint,
        "/api/control/shutdown",
        &secrets.auth_token,
        "POST",
    )?;
    for _ in 0..50 {
        if !http::healthy(&state.endpoint, &secrets.auth_token) {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }
    Err(CliError::Control(
        "Forge accepted shutdown but remained reachable".into(),
    ))
}

#[cfg(test)]
mod tests {
    use std::{
        ffi::OsStr,
        path::{Path, PathBuf},
    };

    use super::{
        Command, InstanceConfig, InstancePaths, Secrets, configure_forge_environment,
        configure_native_runtime,
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
        configure_forge_environment(&mut command, &paths, &config, &secrets, forge_root);
        assert!(
            command
                .get_envs()
                .all(|(key, _)| key != OsStr::new("ARTISAN_STATIC_FRONTEND_ROOT"))
        );

        let (paths, config, secrets) = test_instance(true);
        let mut serving = Command::new("forge");
        configure_forge_environment(&mut serving, &paths, &config, &secrets, forge_root);
        assert!(serving.get_envs().any(|(key, value)| {
            key == OsStr::new("ARTISAN_STATIC_FRONTEND_ROOT")
                && value.is_some_and(|path| Path::new(path).ends_with("frontend"))
        }));
    }

    #[test]
    fn exposes_packaged_native_modules_to_forge_node_resolution() {
        let native_runtime = Path::new("C:/Artisan/forge/native-runtime");
        let mut command = Command::new("forge");

        configure_native_runtime(&mut command, native_runtime);

        let environment = command.get_envs().collect::<Vec<_>>();
        for name in ["ARTISAN_NATIVE_RUNTIME", "NODE_PATH"] {
            assert!(environment.iter().any(|(key, value)| {
                *key == OsStr::new(name) && value.as_deref() == Some(native_runtime.as_os_str())
            }));
        }
    }
}
