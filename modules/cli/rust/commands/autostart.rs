use std::{
    fs,
    process::{Command, Stdio},
    time::Instant,
};

use crate::{
    CliError, Result,
    credentials::{self, ForgeCredentialPaths},
    error::io,
    host_access::{self, HostAccess},
    instance::{self, NativeInstanceConfig},
    paths::Layout,
    payload, process, telemetry,
};

use super::{
    FORGE_READY_TIMEOUT, NativeSetupValues, native_launch_spec, require_installation,
    require_launchable_installation,
};

#[cfg(any(not(target_os = "linux"), test))]
mod logon_task;
#[cfg(target_os = "linux")]
mod systemd;

#[cfg(test)]
pub(super) use logon_task::{
    ScheduledTaskDeletion, StableLauncher, scheduled_task_action, scheduled_task_create_args,
    scheduled_task_deletion, stable_launcher_kind,
};

pub(super) fn setup_native(layout: &Layout, values: NativeSetupValues) -> Result<()> {
    // The Forge creates its files, not their directories.
    for runtime_path in [
        &values.database_path,
        &values.custody_path,
        &values.readiness_path,
    ] {
        if let Some(parent) = runtime_path.parent() {
            fs::create_dir_all(parent).map_err(io("create Forge runtime directory"))?;
        }
    }
    let credential_paths = ForgeCredentialPaths::from_home(&layout.root)?;
    let instance_path = layout.native_instance_path();
    let instance_id = match fs::symlink_metadata(&instance_path) {
        Ok(_) => NativeInstanceConfig::load(&instance_path)?.instance_id(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => instance::mint_instance_id()?,
        Err(source) => {
            return Err(CliError::Io {
                context: "inspect native Forge instance",
                source,
            });
        }
    };
    let config = NativeInstanceConfig::new_with_instance_id(
        instance_id,
        values.database_path,
        values.custody_path,
        values.readiness_path,
        credential_paths.manifest_path().to_path_buf(),
        values.listener,
        values.native_run,
    )?;
    fs::create_dir_all(&layout.root).map_err(io("create Artisan home directory"))?;
    let provisioned = credentials::provision_or_load(&layout.root)?;
    process::validate_credential_manifest(&config, &provisioned)?;
    config.write_to_home(&layout.root)?;
    Ok(())
}

/// Starts the Forge: in the foreground as a supervisor (what the Linux
/// service runs), through the user's service manager when the installation
/// has one, or detached. A Forge with host access publishes its invitation
/// once ready.
pub(super) fn start(layout: &Layout, foreground: bool) -> Result<process::StartResult> {
    let manifest = require_launchable_installation(layout)?;
    telemetry::load_or_create(layout)?;
    payload::require_verified(&manifest.version_root())?;
    let access = HostAccess::load(&layout.root)?;
    let spec = native_launch_spec(layout)?;
    let deadline = Instant::now() + FORGE_READY_TIMEOUT;
    let publish = |readiness: &process::ForgeReadiness| match &access {
        Some(access) => host_access::publish_invitation(&layout.root, access, readiness).map(drop),
        None => Ok(()),
    };
    if foreground {
        return process::supervise(&spec, deadline, &mut |readiness| publish(readiness));
    }
    #[cfg(target_os = "linux")]
    if let Some(result) = systemd::start(layout, &spec, deadline)? {
        return Ok(result);
    }
    let result = process::start_until(&spec, false, deadline)?;
    if let process::ForgeReadinessStatus::Ready(readiness) =
        process::readiness_status(spec.readiness_path(), spec.executable())
    {
        publish(&readiness)?;
    }
    Ok(result)
}

pub(super) fn unsupported_lifecycle_control() -> Result<()> {
    Err(CliError::UnsupportedLifecycleControl)
}

pub(super) fn autostart(layout: &Layout, disable: bool) -> Result<()> {
    if disable {
        disable_autostart(layout)?;
        println!("disabled");
    } else {
        println!(
            "{}",
            if autostart_enabled(layout)? {
                "enabled"
            } else {
                "disabled"
            }
        );
    }
    Ok(())
}

/// Starts the Forge with the user's session: the systemd user service on
/// Linux, the logon task on Windows. `configuration_changed` restarts a
/// running Linux service so a new configuration takes effect.
pub(super) fn enable_autostart(layout: &Layout, configuration_changed: bool) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        systemd::enable(layout, configuration_changed)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = configuration_changed;
        logon_task::enable(layout)
    }
}

/// Stops a service-managed Forge `pid` through the user manager; `false`
/// when the installation's Forge is not a running service.
pub(super) fn stop_service(layout: &Layout, pid: u32) -> Result<bool> {
    #[cfg(target_os = "linux")]
    {
        systemd::stop(layout, pid)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (layout, pid);
        Ok(false)
    }
}

fn disable_autostart(layout: &Layout) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        systemd::disable(layout)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = layout;
        logon_task::disable()
    }
}

fn autostart_enabled(layout: &Layout) -> Result<bool> {
    #[cfg(target_os = "linux")]
    {
        systemd::enabled(layout)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = layout;
        logon_task::enabled()
    }
}

pub(super) fn delegate_installer(
    layout: &Layout,
    operation: &str,
    remove_data: bool,
) -> Result<()> {
    if operation == "update" {
        require_launchable_installation(layout)?;
    }
    let manifest = require_installation(layout)?;
    let bootstrap = manifest.installer_executable();
    if !bootstrap.is_file() {
        return Err(CliError::Installation(format!(
            "installer lifecycle binary is missing at {}; reinstall Artisan",
            bootstrap.display()
        )));
    }
    let mut command = Command::new(bootstrap);
    command
        .arg(operation)
        .arg("--install-root")
        .arg(&manifest.install_root);
    if remove_data {
        command.arg("--remove-data");
    }
    if operation == "diagnose" {
        command.stdout(Stdio::null()).stderr(Stdio::null());
    }
    let status = command.status().map_err(io("run installer lifecycle"))?;
    if status.success() {
        Ok(())
    } else {
        Err(CliError::Installation(format!(
            "{operation} failed with {status}"
        )))
    }
}
