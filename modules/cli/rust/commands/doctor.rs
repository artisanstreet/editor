use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    thread,
    time::Duration,
};

use serde::Serialize;

use crate::{
    CliError, Result,
    credentials::{self, ForgeCredentialError, ForgeCredentialPaths},
    error::io,
    instance::{self, NativeInstanceConfig},
    manifest::{InstallationFinalization, InstallationManifest},
    paths::Layout,
    payload,
};

use super::autostart::delegate_installer;
use super::load_native_instance;

const MAX_LOG_BYTES: u64 = 1024 * 1024;

const MAX_FOLLOW_BYTES: u64 = 64 * 1024;

pub(super) fn logs(layout: &Layout, lines: usize, follow: bool) -> Result<()> {
    let (paths, _, _) = instance::load(layout)?;
    let mut file = File::open(&paths.log).map_err(io("open Forge log"))?;
    let size = file.metadata().map_err(io("inspect Forge log"))?.len();
    file.seek(SeekFrom::Start(size.saturating_sub(MAX_LOG_BYTES)))
        .map_err(io("seek Forge log"))?;
    let mut bytes = Vec::new();
    file.by_ref()
        .take(MAX_LOG_BYTES)
        .read_to_end(&mut bytes)
        .map_err(io("read Forge log"))?;
    let text = String::from_utf8_lossy(&bytes);
    let selected = text
        .lines()
        .rev()
        .take(lines.clamp(1, 10_000))
        .collect::<Vec<_>>();
    for line in selected.into_iter().rev() {
        println!("{line}");
    }
    if follow {
        follow_log(file, size)?;
    }
    Ok(())
}

fn follow_log(mut file: File, mut offset: u64) -> Result<()> {
    loop {
        let size = file.metadata().map_err(io("inspect Forge log"))?.len();
        if size < offset {
            file.seek(SeekFrom::Start(0))
                .map_err(io("seek rotated Forge log"))?;
            offset = 0;
        }
        if size > offset {
            file.seek(SeekFrom::Start(offset))
                .map_err(io("seek Forge log"))?;
            let mut bytes = Vec::new();
            file.by_ref()
                .take((size - offset).min(MAX_FOLLOW_BYTES))
                .read_to_end(&mut bytes)
                .map_err(io("follow Forge log"))?;
            offset += bytes.len() as u64;
            print!("{}", String::from_utf8_lossy(&bytes));
        }
        thread::sleep(Duration::from_millis(200));
    }
}

pub(super) fn doctor(
    layout: &Layout,
    fix: bool,
    json: bool,
    finalization_check: bool,
) -> Result<()> {
    if finalization_check {
        return doctor_finalization(layout);
    }
    if fix {
        delegate_installer(layout, "repair", false)?;
    }
    let installation = InstallationManifest::load(&layout.manifest);
    let protocol = if installation.is_ok() {
        delegate_installer(layout, "diagnose", false)
    } else {
        Err(CliError::Installation(
            "protocol health is unavailable without a valid installation".to_owned(),
        ))
    };
    // The Forge instance is the native configuration `ae setup` writes and
    // `ae start` runs, not the retired pre-native instance files.
    let (instance_state, _) = inspect_native_instance(layout);
    // Payload drift (for example a development build copied over an installed
    // version) is reported, never repaired. Versions installed before payload
    // manifests existed stay honestly unverifiable without failing doctor.
    let payload_health = installation.as_ref().map_or(
        payload::PayloadHealth::Unverifiable,
        |manifest: &InstallationManifest| payload::verify(&manifest.version_root()),
    );
    #[cfg(target_os = "linux")]
    let service = Some(service_health(layout));
    #[cfg(not(target_os = "linux"))]
    let service: Option<(&'static str, String)> = None;
    // Repair never invents a Forge configuration. `ae setup` is the sole
    // explicit creator.
    let healthy = installation.is_ok()
        && protocol.is_ok()
        && instance_state == "ok"
        && !matches!(payload_health, payload::PayloadHealth::Modified(_))
        && service
            .as_ref()
            .is_none_or(|(state, _)| matches!(*state, "ok" | "absent"));
    if json {
        println!(
            "{}",
            serde_json::json!({
                "healthy": healthy,
                "installation": if installation.is_ok() { "ok" } else { "error" },
                "protocol": if protocol.is_ok() { "ok" } else { "error" },
                "instance": instance_state,
                "payload": payload_health.as_str(),
                "payload_issues": match &payload_health {
                    payload::PayloadHealth::Modified(issues) => issues.clone(),
                    _ => Vec::new(),
                },
                "service": service.as_ref().map(|(state, _)| *state),
            })
        );
    } else {
        println!(
            "{}: installation",
            if installation.is_ok() { "ok" } else { "error" }
        );
        println!(
            "{}: artisan:// protocol",
            if protocol.is_ok() { "ok" } else { "error" }
        );
        match instance_state {
            "ok" => println!("ok: forge instance"),
            state => println!("error: forge instance ({state}; run `ae setup`)"),
        }
        match &payload_health {
            payload::PayloadHealth::Verified => println!("ok: version payload"),
            payload::PayloadHealth::Modified(issues) => {
                println!("error: version payload modified ({})", issues.join(", "));
            }
            payload::PayloadHealth::Unverifiable => {
                println!("warn: version payload (unverifiable: no payload manifest)");
            }
        }
        if let Some((state, detail)) = &service {
            let level = match *state {
                "ok" => "ok",
                "absent" => "warn",
                _ => "error",
            };
            println!("{level}: forge service ({detail})");
        }
    }
    if healthy {
        Ok(())
    } else {
        Err(CliError::Installation(
            "doctor found unresolved issues".into(),
        ))
    }
}

/// The Linux Forge service: its state (`ok`, `absent`, `drifted`,
/// `foreign`, or `unavailable`) and a human detail. Other platforms have no
/// service line.
#[cfg(target_os = "linux")]
fn service_health(layout: &Layout) -> (&'static str, String) {
    use crate::service::{ForgeService, UserSystemctl};

    match ForgeService::for_current_user(&layout.root) {
        Ok(service) => {
            let health = service.health(&UserSystemctl);
            (health.state(), service_detail(&service, &health))
        }
        Err(error) => ("unavailable", error.to_string()),
    }
}

/// One doctor line's detail for `health` of `service`.
#[cfg(any(target_os = "linux", test))]
fn service_detail(
    service: &crate::service::ForgeService,
    health: &crate::service::ServiceHealth,
) -> String {
    use crate::service::ServiceHealth;

    let name = service.unit_name();
    let known = |value: Option<bool>, yes: &'static str, no: &'static str| match value {
        Some(true) => yes,
        Some(false) => no,
        None => "unknown",
    };
    match health {
        ServiceHealth::Absent => format!("{name} is not installed; run `ae setup --autostart`"),
        ServiceHealth::Foreign => format!(
            "{} was not written for this installation",
            service.unit_path().display()
        ),
        ServiceHealth::Drifted => {
            format!("{name} no longer runs this installation; run `ae setup --autostart`")
        }
        ServiceHealth::Installed { enabled, active } => format!(
            "{name} {}, {}",
            known(*enabled, "enabled", "disabled"),
            known(*active, "active", "inactive")
        ),
        ServiceHealth::Unavailable(reason) => reason.clone(),
    }
}

#[derive(Serialize)]
pub(super) struct DoctorFinalizationReport {
    pub(super) schema: &'static str,
    pub(super) healthy: bool,
    pub(super) finalization: &'static str,
    pub(super) installation: &'static str,
    pub(super) protocol: &'static str,
    pub(super) instance: &'static str,
    pub(super) credentials: &'static str,
    pub(super) payload: &'static str,
    pub(super) payload_issues: Vec<DoctorPayloadIssue>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum DoctorPayloadIssue {
    Missing,
    Modified,
    Unreadable,
    Unexpected,
    Invalid,
}

fn doctor_finalization(layout: &Layout) -> Result<()> {
    let report = doctor_finalization_report(layout);
    let healthy = report.healthy;
    println!(
        "{}",
        serde_json::to_string(&report)
            .map_err(|_| CliError::Installation("could not serialize doctor report".into()))?
    );
    if healthy {
        Ok(())
    } else {
        Err(CliError::Installation(
            "doctor found unresolved issues".into(),
        ))
    }
}

fn doctor_finalization_report(layout: &Layout) -> DoctorFinalizationReport {
    let (finalization, installation) = InstallationManifest::inspect(&layout.manifest);
    let installation_state = if installation.is_some() {
        "ok"
    } else {
        "error"
    };
    let (instance_state, instance) = inspect_native_instance(layout);
    let credentials_state = instance.as_ref().map_or("not_checked", |instance| {
        inspect_existing_credentials(layout, instance)
    });
    let (payload_state, payload_issues) = inspect_payload(installation.as_ref());
    let healthy = installation_state == "ok"
        && matches!(
            finalization,
            InstallationFinalization::Complete | InstallationFinalization::Pending
        )
        && instance_state == "ok"
        && credentials_state == "ok"
        && payload_state == "ok";
    DoctorFinalizationReport {
        schema: "artisan-doctor-finalization-v1",
        healthy,
        finalization: finalization.as_str(),
        installation: installation_state,
        protocol: "deferred",
        instance: instance_state,
        credentials: credentials_state,
        payload: payload_state,
        payload_issues,
    }
}

pub(super) fn inspect_native_instance(
    layout: &Layout,
) -> (&'static str, Option<NativeInstanceConfig>) {
    match load_native_instance(layout) {
        Ok(instance) => ("ok", Some(instance)),
        Err(CliError::MissingInstance) => ("missing", None),
        Err(_) => ("invalid", None),
    }
}

pub(super) fn inspect_existing_credentials(
    layout: &Layout,
    instance: &NativeInstanceConfig,
) -> &'static str {
    let Ok(paths) = ForgeCredentialPaths::from_home(&layout.root) else {
        return "invalid";
    };
    if instance.credentials_manifest() != paths.manifest_path() {
        return "invalid";
    }
    match credentials::load_existing_client_identity(&layout.root) {
        Ok(identity) if identity.paths().manifest_path() == instance.credentials_manifest() => "ok",
        Err(ForgeCredentialError::IdentityBundleMissing) => "missing",
        Ok(_) | Err(_) => "invalid",
    }
}

fn inspect_payload(
    installation: Option<&InstallationManifest>,
) -> (&'static str, Vec<DoctorPayloadIssue>) {
    let Some(installation) = installation else {
        return ("not_checked", Vec::new());
    };
    match payload::verify(&installation.version_root()) {
        payload::PayloadHealth::Verified => ("ok", Vec::new()),
        payload::PayloadHealth::Modified(issues) => ("modified", payload_issue_codes(&issues)),
        payload::PayloadHealth::Unverifiable => ("unverifiable", Vec::new()),
    }
}

pub(super) fn payload_issue_codes(issues: &[String]) -> Vec<DoctorPayloadIssue> {
    let mut codes = Vec::new();
    for issue in issues {
        let code = if issue.starts_with("missing") {
            DoctorPayloadIssue::Missing
        } else if issue.starts_with("modified") {
            DoctorPayloadIssue::Modified
        } else if issue.starts_with("unreadable") {
            DoctorPayloadIssue::Unreadable
        } else if issue.starts_with("unexpected") {
            DoctorPayloadIssue::Unexpected
        } else {
            DoctorPayloadIssue::Invalid
        };
        if !codes.contains(&code) {
            codes.push(code);
        }
    }
    codes
}

#[cfg(test)]
mod service_tests {
    use std::path::Path;

    use super::service_detail;
    use crate::service::{ForgeService, ServiceHealth, UserDirectories};

    #[test]
    fn doctor_names_the_unit_and_what_to_do_for_every_state() {
        let service = ForgeService::for_root(
            Path::new("/home/ada/.local/share/Artisan Street Dev"),
            &UserDirectories {
                home: Some("/home/ada".into()),
                ..UserDirectories::default()
            },
        )
        .expect("service");
        let detail = |health: ServiceHealth| {
            (
                health.state(),
                health.is_healthy(),
                service_detail(&service, &health),
            )
        };
        assert_eq!(
            detail(ServiceHealth::Installed {
                enabled: Some(true),
                active: Some(true)
            }),
            (
                "ok",
                true,
                "artisan-forge-dev.service enabled, active".to_owned()
            )
        );
        let (state, healthy, text) = detail(ServiceHealth::Absent);
        assert_eq!((state, healthy), ("absent", true));
        assert!(text.contains("ae setup --autostart"));
        for (health, state) in [
            (ServiceHealth::Drifted, "drifted"),
            (ServiceHealth::Foreign, "foreign"),
            (
                ServiceHealth::Unavailable("no manager".to_owned()),
                "unavailable",
            ),
        ] {
            let (reported, healthy, _) = detail(health);
            assert_eq!(reported, state);
            assert!(!healthy, "{state} makes doctor fail");
        }
    }
}
