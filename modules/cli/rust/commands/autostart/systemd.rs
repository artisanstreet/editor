//! Linux autostart: the installation's systemd user service.
//!
//! On Linux `ae setup --autostart` installs and enables the unit
//! [`crate::service`] renders, and `ae start` hands a service-managed Forge
//! to the user manager instead of spawning a second, unsupervised one.

use std::{
    thread,
    time::{Duration, Instant},
};

use crate::{
    CliError, Result,
    paths::Layout,
    process::{self, ForgeLaunchSpec, ForgeReadinessStatus, StartResult},
    service::{ForgeService, Systemctl, UnitFile, UserSystemctl},
};

const READINESS_POLL: Duration = Duration::from_millis(100);

/// Installs and enables the service; a running service restarts when its
/// configuration changed, so the new configuration takes effect.
pub(super) fn enable(layout: &Layout, configuration_changed: bool) -> Result<()> {
    let service = ForgeService::for_current_user(&layout.root)?;
    let path = std::env::var("PATH").ok();
    let unit_changed = service.install(&UserSystemctl, path.as_deref())?;
    if unit_changed || configuration_changed {
        service.try_restart(&UserSystemctl)?;
    }
    println!("Forge service {} enabled", service.unit_name());
    Ok(())
}

/// Stops the service and removes its unit.
pub(super) fn disable(layout: &Layout) -> Result<()> {
    ForgeService::for_current_user(&layout.root)?.remove(&UserSystemctl)
}

/// Whether the installation's service is installed and enabled.
pub(super) fn enabled(layout: &Layout) -> Result<bool> {
    let service = ForgeService::for_current_user(&layout.root)?;
    Ok(matches!(service.inspect()?, UnitFile::Owned { .. })
        && service.is_enabled(&UserSystemctl)?)
}

/// Starts a service-managed Forge through the user manager and waits for its
/// readiness. `None` when the installation has no service: the caller
/// starts the Forge itself.
pub(super) fn start(
    layout: &Layout,
    spec: &ForgeLaunchSpec,
    deadline: Instant,
) -> Result<Option<StartResult>> {
    let service = ForgeService::for_current_user(&layout.root)?;
    if !matches!(service.inspect()?, UnitFile::Owned { .. }) {
        return Ok(None);
    }
    start_and_wait(&service, &UserSystemctl, spec, deadline).map(Some)
}

fn start_and_wait(
    service: &ForgeService,
    systemctl: &dyn Systemctl,
    spec: &ForgeLaunchSpec,
    deadline: Instant,
) -> Result<StartResult> {
    if let ForgeReadinessStatus::Ready(_) =
        process::readiness_status(spec.readiness_path(), spec.executable())
    {
        return Ok(StartResult::AlreadyRunning);
    }
    service.start(systemctl)?;
    loop {
        if let ForgeReadinessStatus::Ready(readiness) =
            process::readiness_status(spec.readiness_path(), spec.executable())
        {
            return Ok(StartResult::Spawned {
                pid: readiness.pid(),
            });
        }
        if !service.is_active(systemctl)? {
            return Err(CliError::Service(format!(
                "{} stopped before its Forge became ready; see `journalctl --user -u {}`",
                service.unit_name(),
                service.unit_name()
            )));
        }
        if Instant::now() >= deadline {
            return Err(CliError::ForgeReadinessTimeout);
        }
        thread::sleep(READINESS_POLL);
    }
}
