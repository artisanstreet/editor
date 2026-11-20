use crate::{install, platform, service, wsl};
use artisan_api::{EngineId, InstallState};
use artisan_paths::App;
use forge_store::admin::{Admin, AdminError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Report {
    #[serde(default)]
    machine: String,
    #[serde(default)]
    apps: Vec<AppReport>,
    #[serde(default)]
    forge: Option<ForgeReport>,
    #[serde(default)]
    problems: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct AppReport {
    app: String,
    version: String,
    path: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ForgeReport {
    running: bool,
    pid: Option<u32>,
    address: Option<String>,
    listening: bool,
    healthy: Option<bool>,
    log: Option<String>,
    devices: Option<usize>,
    #[serde(default)]
    engines: Vec<EngineReport>,
}

#[derive(Debug, Serialize, Deserialize)]
struct EngineReport {
    engine: String,
    version: Option<String>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum Remote {
    Read(Report),
    Unreadable { error: String },
}

#[derive(Serialize)]
struct Everything<'a> {
    #[serde(flatten)]
    local: &'a Report,
    wsl: &'a BTreeMap<String, Remote>,
}

impl Report {
    pub(crate) fn has(&self, app: App) -> bool {
        self.apps.iter().any(|installed| installed.app == app.id())
    }
}

pub(crate) fn status(json_output: bool) {
    let report = local();
    let wsl: BTreeMap<String, Remote> = if cfg!(windows) {
        match wsl::probe_all() {
            Ok(probes) => probes
                .into_iter()
                .filter(|probe| probe.report.is_err() || probe.has_forge())
                .map(|probe| {
                    let remote = probe.report.map_or_else(
                        |error| Remote::Unreadable {
                            error: diagnostics::chain(&error),
                        },
                        Remote::Read,
                    );
                    (probe.distro, remote)
                })
                .collect(),
            Err(error) => BTreeMap::from([(
                "WSL".to_owned(),
                Remote::Unreadable {
                    error: diagnostics::chain(&error),
                },
            )]),
        }
    } else {
        BTreeMap::new()
    };
    if json_output {
        let everything = Everything {
            local: &report,
            wsl: &wsl,
        };
        crate::ui::print(
            serde_json::to_string(&everything)
                .expect("invariant: the status report serializes to JSON"),
        );
        return;
    }
    print(&report, None);
    for (distro, remote) in &wsl {
        crate::ui::print("");
        match remote {
            Remote::Read(report) => print(report, Some(distro)),
            Remote::Unreadable { error } => {
                crate::ui::print(format_args!("{distro} (WSL)"));
                crate::ui::print(format_args!("  ! couldn't read its status: {error}"));
            }
        }
    }
}

fn paired_devices(problems: &mut Vec<String>) -> Option<usize> {
    let counted = App::Forge
        .data()
        .map_err(|error| diagnostics::chain(&error))
        .and_then(|data| match Admin::open_read_only(&data) {
            Ok(admin) => admin
                .devices()
                .map(|devices| devices.len())
                .map_err(|error| diagnostics::chain(&error)),
            Err(AdminError::Missing { .. }) => Ok(0),
            Err(error) => Err(diagnostics::chain(&error)),
        });
    match counted {
        Ok(count) => Some(count),
        Err(error) => {
            problems.push(format!("Couldn't count paired devices: {error}"));
            None
        }
    }
}

#[cfg(unix)]
fn health(problems: &mut Vec<String>, log: Option<&str>) -> bool {
    let checked = App::Forge
        .data()
        .map_err(|error| diagnostics::chain(&error))
        .and_then(|data| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| {
                    format!(
                        "start the health check runtime: {}",
                        diagnostics::chain(&error)
                    )
                })?
                .block_on(forge_http::upgrade::health(&data))
                .map_err(|error| diagnostics::chain(&error))
        });
    match checked {
        Ok(_) => true,
        Err(error) => {
            problems.push(format!(
                "Forge isn't healthy: {error}; see {}",
                log.unwrap_or("its log")
            ));
            false
        }
    }
}

fn local() -> Report {
    let mut problems = Vec::new();
    let mut apps = Vec::new();
    let mut present = Vec::new();
    for app in App::ALL {
        let version = match install::installed(app) {
            Ok(install::Installed::Present(version)) => version,
            Ok(install::Installed::Absent) => continue,
            Err(error) => {
                problems.push(format!(
                    "Couldn't tell whether {} is installed: {}",
                    app.id(),
                    diagnostics::chain(&error)
                ));
                continue;
            }
        };
        present.push(app);
        match &version {
            install::InstalledVersion::Known(_) => {}
            install::InstalledVersion::Missing => problems.push(format!(
                "{} has no version file; run `ae install` to repair it",
                app.id()
            )),
            install::InstalledVersion::Unreadable { path, kind } => problems.push(format!(
                "{}'s version file {} can't be read ({kind}); run `ae install` to repair it",
                app.id(),
                path.display()
            )),
        }
        match app.dir() {
            Ok(dir) => apps.push(AppReport {
                app: app.id().to_owned(),
                version: version.to_string(),
                path: dir.display().to_string(),
            }),
            Err(error) => problems.push(format!(
                "Couldn't find {}'s folder: {}",
                app.id(),
                diagnostics::chain(&error)
            )),
        }
    }
    if present.contains(&App::Ae) {
        match platform::path_ok() {
            Ok(true) => {}
            Ok(false) => {
                problems.push("`ae` isn't on PATH for new terminals; run `ae install`".to_owned());
            }
            Err(error) => problems.push(format!(
                "Couldn't check whether `ae` is on PATH: {}",
                diagnostics::chain(&error)
            )),
        }
    }
    let forge = present.contains(&App::Forge).then(|| forge(&mut problems));

    if present.is_empty() {
        problems.push("Nothing is installed here; run `ae install`".to_owned());
    }
    Report {
        machine: machine_name(),
        apps,
        forge,
        problems,
    }
}

fn forge(problems: &mut Vec<String>) -> ForgeReport {
    let running = match service::is_running() {
        Ok(running) => running,
        Err(error) => {
            problems.push(format!(
                "Couldn't tell whether Forge is running: {}",
                diagnostics::chain(&error)
            ));
            false
        }
    };
    let address = match service::address() {
        Ok(address) => Some(address),
        Err(error) => {
            problems.push(diagnostics::chain(&error));
            None
        }
    };
    let listening = address.is_some_and(service::listening);
    let log = match service::log_hint() {
        Ok(log) => Some(log),
        Err(error) => {
            problems.push(format!(
                "Couldn't find Forge's log: {}",
                diagnostics::chain(&error)
            ));
            None
        }
    };
    #[cfg(unix)]
    let healthy = running.then(|| health(problems, log.as_deref()));
    #[cfg(not(unix))]
    let healthy = None;
    if !running {
        problems.push("Forge isn't running; run `ae forge start`".to_owned());
    } else if let Some(address) = address.filter(|_| !listening) {
        problems.push(format!(
            "Forge runs but doesn't answer on {address}; see {}",
            log.as_deref().unwrap_or("its log")
        ));
    }
    let devices = paired_devices(problems);
    let pid = match service::running_pid() {
        Ok(pid) => pid,
        Err(error) => {
            problems.push(format!(
                "Couldn't read which process Forge runs as: {}",
                diagnostics::chain(&error)
            ));
            None
        }
    };
    let engines: Vec<EngineReport> = match App::Forge.data() {
        Ok(data) => {
            let engines = forge_engines::Engines::new(&data);
            EngineId::ALL
                .iter()
                .map(|engine| {
                    let version = match engines.status(*engine).install {
                        InstallState::Installed { version, .. } => Some(version),
                        InstallState::Absent
                        | InstallState::Downloading { .. }
                        | InstallState::Verifying { .. }
                        | InstallState::Failed { .. } => None,
                    };
                    EngineReport {
                        engine: engine.as_str().to_owned(),
                        version,
                    }
                })
                .collect()
        }
        Err(error) => {
            problems.push(format!(
                "Couldn't list Forge's engines: {}",
                diagnostics::chain(&error)
            ));
            Vec::new()
        }
    };
    ForgeReport {
        running,
        pid,
        address: address.map(|address| address.to_string()),
        listening,
        healthy,
        log,
        devices,
        engines,
    }
}

fn machine_name() -> String {
    if platform::in_wsl() {
        format!(
            "{} (WSL)",
            std::env::var("WSL_DISTRO_NAME").unwrap_or_default()
        )
    } else if cfg!(windows) {
        "This PC".to_owned()
    } else {
        "This machine".to_owned()
    }
}

fn print(report: &Report, distro: Option<&str>) {
    let machine = distro.map_or_else(
        || report.machine.clone(),
        |distro| format!("{distro} (WSL)"),
    );
    crate::ui::print(&machine);
    for app in &report.apps {
        let name = match app.app.as_str() {
            "forge" => "Forge",
            "editor" => "Editor",
            _ => "ae",
        };
        crate::ui::print(format_args!("  {name:<8} {:<16} {}", app.version, app.path));
    }
    if let Some(forge) = &report.forge {
        let state = if forge.running {
            let pid = forge
                .pid
                .map(|pid| format!(", pid {pid}"))
                .unwrap_or_default();
            let health = if forge.healthy == Some(false) {
                ", unhealthy"
            } else {
                ""
            };
            match &forge.address {
                Some(address) => format!("● running on {address}{pid}{health}"),
                None => format!("● running{pid}{health}"),
            }
        } else {
            "○ stopped".to_owned()
        };
        crate::ui::print(format_args!("  Forge    {state}"));
        crate::ui::print(format_args!(
            "           log: {}",
            forge.log.as_deref().unwrap_or("unknown")
        ));
        match forge.devices {
            Some(devices) => crate::ui::print(format_args!(
                "           {}",
                crate::ui::count(devices as u64, "paired device", "paired devices")
            )),
            None => crate::ui::print("           paired devices unknown"),
        }
        let engines: Vec<String> = forge
            .engines
            .iter()
            .map(|engine| {
                format!(
                    "{} {}",
                    engine.engine,
                    engine.version.as_deref().unwrap_or("not installed")
                )
            })
            .collect();
        crate::ui::print(format_args!("           engines: {}", engines.join(", ")));
    }
    for problem in &report.problems {
        crate::ui::print(format_args!("  ! {problem}"));
    }
}
