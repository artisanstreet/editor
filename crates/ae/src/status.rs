//! `ae status`: what is installed where, whether Forge runs and answers,
//! and what to do about problems. On Windows it includes Forge in WSL.

use crate::{install, platform, service, wsl};
use artisan_api::{EngineId, InstallState};
use artisan_paths::App;
use serde_json::{Value, json};

pub fn status(json_output: bool) {
    let report = local();
    let wsl: Vec<(String, Value)> = if cfg!(windows) {
        wsl::distros_with_forge()
            .into_iter()
            .filter_map(|distro| {
                let output = wsl::ae(&distro).args(["status", "--json"]).output().ok()?;
                let value = serde_json::from_slice(&output.stdout).ok()?;
                Some((distro, value))
            })
            .collect()
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
        let address = service::address();
        let running = service::is_running();
        let listening = service::listening(address);
        if !running {
            problems.push("Forge isn't running; run `ae forge start`".to_owned());
        } else if !listening {
            problems.push(format!(
                "Forge runs but doesn't answer on {address}; see {}",
                service::log_hint()
            ));
        }
        let data = App::Forge.data().ok();
        let devices = data
            .as_ref()
            .filter(|data| data.join("forge.sqlite3").exists())
            .and_then(|data| forge_store::admin::Admin::open(data).ok())
            .and_then(|admin| admin.devices().ok())
            .map_or(0, |devices| devices.len());
        let engines: Vec<Value> = data
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
            "address": address.to_string(),
            "listening": listening,
            "log": service::log_hint(),
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
            format!(
                "● running on {}{pid}",
                forge["address"].as_str().unwrap_or_default()
            )
        } else {
            "○ stopped".to_owned()
        };
        println!("  Forge    {state}");
        println!(
            "           log: {}",
            forge["log"].as_str().unwrap_or_default()
        );
        println!("           {} paired device(s)", forge["devices"]);
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
