use crate::{install, platform, service, wsl};
use artisan_api::{EngineId, InstallState};
use artisan_paths::App;
use forge_store::admin::{Admin, AdminError};
use serde_json::{Value, json};

pub fn status(json_output: bool) {
    let report = local();
    let wsl: Vec<(String, Value)> = if cfg!(windows) {
        match wsl::probe_all() {
            Ok(probes) => probes
                .into_iter()
                .filter(|probe| probe.report.is_err() || probe.has_forge())
                .map(|probe| {
                    let report = probe
                        .report
                        .unwrap_or_else(|error| json!({ "error": format!("{error:#}") }));
                    (probe.distro, report)
                })
                .collect(),
            Err(error) => vec![("WSL".to_owned(), json!({ "error": format!("{error:#}") }))],
        }
    } else {
        Vec::new()
    };
    if json_output {
        let mut value = report;
        value["wsl"] = Value::Object(wsl.into_iter().collect());
        println!("{value}");
        return;
    }
    print(&report, None);
    for (distro, report) in &wsl {
        println!();
        print(report, Some(distro));
    }
}

fn paired_devices(problems: &mut Vec<String>) -> Value {
    let counted = App::Forge
        .data()
        .map_err(|error| format!("{error}"))
        .and_then(|data| match Admin::open_read_only(&data) {
            Ok(admin) => admin
                .devices()
                .map(|devices| devices.len())
                .map_err(|error| format!("{error:#}")),
            Err(AdminError::Missing { .. }) => Ok(0),
            Err(error) => Err(diagnostics::chain(&error)),
        });
    match counted {
        Ok(count) => json!(count),
        Err(error) => {
            problems.push(format!("Couldn't count paired devices: {error}"));
            Value::Null
        }
    }
}

#[cfg(unix)]
fn health(problems: &mut Vec<String>, log: Option<&str>) -> Value {
    let checked = App::Forge
        .data()
        .map_err(|error| format!("{error}"))
        .and_then(|data| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| format!("start the health check runtime: {error}"))?
                .block_on(forge_http::upgrade::health(&data))
                .map_err(|error| diagnostics::chain(&error))
        });
    match checked {
        Ok(_) => json!(true),
        Err(error) => {
            problems.push(format!(
                "Forge isn't healthy: {error}; see {}",
                log.unwrap_or("its log")
            ));
            json!(false)
        }
    }
}

fn local() -> Value {
    let apps: Vec<Value> = App::ALL
        .iter()
        .filter_map(|app| {
            let version = install::installed(*app)?;
            Some(json!({
                "app": app.id(),
                "version": version,
                "path": app.dir().ok()?.display().to_string(),
            }))
        })
        .collect();
    let mut problems = Vec::new();
    let has = |app: App| install::installed(app).is_some();
    if has(App::Ae) && !platform::path_ok() {
        problems.push("`ae` isn't on PATH for new terminals; run `ae install`".to_owned());
    }
    let forge = if has(App::Forge) {
        let running = service::is_running();
        let address = match service::address() {
            Ok(address) => Some(address),
            Err(error) => {
                problems.push(format!("{error:#}"));
                None
            }
        };
        let listening = address.is_some_and(service::listening);
        let log = match service::log_hint() {
            Ok(log) => Some(log),
            Err(error) => {
                problems.push(format!("Couldn't find Forge's log: {error:#}"));
                None
            }
        };
        #[cfg(unix)]
        let healthy = if running {
            health(&mut problems, log.as_deref())
        } else {
            Value::Null
        };
        #[cfg(not(unix))]
        let healthy = Value::Null;
        if !running {
            problems.push("Forge isn't running; run `ae forge start`".to_owned());
        } else if let Some(address) = address.filter(|_| !listening) {
            problems.push(format!(
                "Forge runs but doesn't answer on {address}; see {}",
                log.as_deref().unwrap_or("its log")
            ));
        }
        let devices = paired_devices(&mut problems);
        let engines: Vec<Value> = App::Forge
            .data()
            .map(|data| {
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
                        json!({"engine": engine.as_str(), "version": version})
                    })
                    .collect()
            })
            .unwrap_or_default();
        json!({
            "running": running,
            "pid": service::running_pid(),
            "address": address.map(|address| address.to_string()),
            "listening": listening,
            "healthy": healthy,
            "log": log,
            "devices": devices,
            "engines": engines,
        })
    } else {
        Value::Null
    };

    if apps.is_empty() {
        problems.push("Nothing is installed here; run `ae install`".to_owned());
    }
    json!({
        "machine": machine_name(),
        "apps": apps,
        "forge": forge,
        "problems": problems,
    })
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

fn print(report: &Value, distro: Option<&str>) {
    let machine = distro.map_or_else(
        || report["machine"].as_str().unwrap_or_default().to_owned(),
        |distro| format!("{distro} (WSL)"),
    );
    println!("{machine}");
    if let Some(error) = report["error"].as_str() {
        println!("  ! couldn't read its status: {error}");
        return;
    }
    for app in report["apps"].as_array().into_iter().flatten() {
        let name = match app["app"].as_str() {
            Some("forge") => "Forge",
            Some("editor") => "Editor",
            _ => "ae",
        };
        println!(
            "  {name:<8} {:<16} {}",
            app["version"].as_str().unwrap_or_default(),
            app["path"].as_str().unwrap_or_default()
        );
    }
    let forge = &report["forge"];
    if !forge.is_null() {
        let state = if forge["running"].as_bool() == Some(true) {
            let pid = forge["pid"]
                .as_u64()
                .map(|pid| format!(", pid {pid}"))
                .unwrap_or_default();
            let health = if forge["healthy"].as_bool() == Some(false) {
                ", unhealthy"
            } else {
                ""
            };
            format!(
                "● running on {}{pid}{health}",
                forge["address"].as_str().unwrap_or_default()
            )
        } else {
            "○ stopped".to_owned()
        };
        println!("  Forge    {state}");
        println!(
            "           log: {}",
            forge["log"].as_str().unwrap_or("unknown")
        );
        match forge["devices"].as_u64() {
            Some(devices) => println!("           {devices} paired device(s)"),
            None => println!("           paired devices unknown"),
        }
        let engines: Vec<String> = forge["engines"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|engine| {
                format!(
                    "{} {}",
                    engine["engine"].as_str().unwrap_or_default(),
                    engine["version"].as_str().unwrap_or("not installed")
                )
            })
            .collect();
        println!("           engines: {}", engines.join(", "));
    }
    for problem in report["problems"].as_array().into_iter().flatten() {
        println!("  ! {}", problem.as_str().unwrap_or_default());
    }
}
