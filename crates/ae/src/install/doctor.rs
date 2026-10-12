//! `ae doctor [--fix]`: checks what a user would trip over and repairs what
//! an install would have set up.

use super::layout::Root;
use super::platform;
use super::sys::Sys;
use crate::service;
use anyhow::Result;
use artisan_api::{EngineId, InstallState};
use std::path::Path;

struct Report {
    fix: bool,
    problems: usize,
}

impl Report {
    fn check(&mut self, label: &str, ok: bool, repair: impl FnOnce() -> Result<()>) {
        if ok {
            println!("ok     {label}");
        } else if !self.fix {
            println!("FAIL   {label}");
            self.problems += 1;
        } else {
            match repair() {
                Ok(()) => println!("fixed  {label}"),
                Err(error) => {
                    println!("FAIL   {label}: {error:#}");
                    self.problems += 1;
                }
            }
        }
    }
}

pub fn doctor(fix: bool, dry_run: bool, data_dir: &Path) -> Result<()> {
    let sys = Sys { dry_run };
    let root = Root::locate()?;
    let mut report = Report { fix, problems: 0 };
    println!("ae {}", crate::LONG_VERSION);

    match root.current() {
        Some(version) => {
            let shipped = root
                .version_dir(&version)
                .join(format!("ae{}", std::env::consts::EXE_SUFFIX));
            report.check(
                &format!("Artisan {version} is installed in {}", root.dir.display()),
                shipped.is_file(),
                || anyhow::bail!("{} is missing; run `ae install` again", shipped.display()),
            );
            let same = std::fs::read(&shipped)
                .is_ok_and(|bytes| std::fs::read(root.ae()).is_ok_and(|stable| stable == bytes));
            report.check(
                &format!("{} is the installed version", root.ae().display()),
                same,
                || root.activate(&version),
            );
            report.check("`ae` is on PATH", platform::path_ok(&root), || {
                platform::add_to_path(sys, &root)
            });
            if let Some(editor) = root.current_editor() {
                report.check(
                    "Editor shortcuts and artisan:// links",
                    platform::editor_integrated(),
                    || platform::integrate_editor(sys, &editor),
                );
            }
            if cfg!(target_os = "linux") && service::systemd_available() {
                let unit = service::unit_path()?;
                let current = std::fs::read_to_string(&unit).unwrap_or_default();
                let custom = *data_dir != crate::paths::forge_data()?;
                let expected = service::unit_text(&root.ae(), custom.then_some(data_dir));
                let exec_ok = current.lines().find(|line| line.starts_with("ExecStart="))
                    == expected.lines().find(|line| line.starts_with("ExecStart="));
                report.check(&format!("Forge service {}", service::UNIT), exec_ok, || {
                    service::install(sys, &root.ae(), data_dir, custom).map(drop)
                });
            }
        }
        None => report.check(
            &format!("Artisan is installed in {}", root.dir.display()),
            false,
            || anyhow::bail!("run `ae install`"),
        ),
    }

    report.check("Forge is running", service::is_running(data_dir), || {
        service::start(data_dir, dry_run)
    });
    let address = crate::forge::default_listen();
    report.check(
        &format!("Forge accepts connections on {address}"),
        service::listening(address),
        || anyhow::bail!("see `ae forge logs`"),
    );

    let database = data_dir.join("forge.sqlite3");
    if database.exists() {
        match forge_store::admin::Admin::open(data_dir).and_then(|admin| admin.devices()) {
            Ok(devices) => println!(
                "info   {} paired device(s); `ae forge pair` adds one",
                devices.len()
            ),
            Err(error) => report.check(
                &format!("Forge's database {}", database.display()),
                false,
                || Err(error),
            ),
        }
    } else {
        println!("info   no Forge data yet in {}", data_dir.display());
    }

    let engines = forge_engines::Engines::new(data_dir);
    for engine in EngineId::ALL {
        let state = match engines.status(engine).install {
            InstallState::Installed { version, .. } => version,
            _ => "not installed".to_owned(),
        };
        println!("info   {:<9} {state}", engine.display_name());
    }

    if report.problems > 0 {
        let hint = if fix {
            ""
        } else {
            "; `ae doctor --fix` repairs what it can"
        };
        anyhow::bail!("{} problem(s) found{hint}", report.problems);
    }
    Ok(())
}
