//! The Linux Forge service: unit naming, rendering, and installation into a
//! user unit directory through a fake `systemctl --user`.

#![cfg(unix)]

use std::{
    cell::RefCell,
    fs,
    path::{Path, PathBuf},
};

use artisan_editor_cli::{
    Result,
    service::{
        ForgeService, ServiceHealth, Systemctl, SystemctlOutput, UnitFile, UserDirectories,
        quote_exec_argument, unit_name_for,
    },
};

/// Records every `systemctl --user` invocation and answers queries from a
/// fixed state.
#[derive(Default)]
struct FakeSystemctl {
    calls: RefCell<Vec<String>>,
    enabled: bool,
    active: bool,
    fail: Option<&'static str>,
}

impl Systemctl for FakeSystemctl {
    fn run(&self, arguments: &[&str]) -> Result<SystemctlOutput> {
        let call = arguments.join(" ");
        self.calls.borrow_mut().push(call.clone());
        let answer = |yes: bool, positive: &str, negative: &str| {
            String::from(if yes { positive } else { negative })
        };
        let stdout = match arguments.first().copied() {
            Some("is-enabled") => answer(self.enabled, "enabled", "disabled"),
            Some("is-active") => answer(self.active, "active", "inactive"),
            _ => String::new(),
        };
        Ok(SystemctlOutput {
            success: self.fail.is_none_or(|failing| !call.starts_with(failing)),
            stdout,
        })
    }
}

fn directories(scratch: &Path) -> UserDirectories {
    UserDirectories {
        home: Some(scratch.join("home")),
        config_home: Some(scratch.join("config")),
        data_home: Some(scratch.join("data")),
    }
}

fn dev_service(scratch: &Path) -> (ForgeService, PathBuf) {
    let root = scratch.join("data").join("Artisan Street Dev");
    let service = ForgeService::for_root(&root, &directories(scratch)).expect("service");
    (service, root)
}

#[test]
fn installations_in_the_data_directory_get_readable_unit_names() {
    let data = Path::new("/home/ada/.local/share");
    assert_eq!(
        unit_name_for(&data.join("Artisan Street"), Some(data)),
        "artisan-forge.service"
    );
    assert_eq!(
        unit_name_for(&data.join("Artisan Street Dev"), Some(data)),
        "artisan-forge-dev.service"
    );
    // Any other root gets a stable, distinct name from its path, so a
    // scratch installation never shares the real one's unit.
    let scratch = unit_name_for(Path::new("/tmp/verify/Artisan Street Dev"), Some(data));
    assert!(scratch.starts_with("artisan-forge-") && scratch.ends_with(".service"));
    assert_ne!(scratch, "artisan-forge-dev.service");
    assert_eq!(
        scratch,
        unit_name_for(Path::new("/tmp/verify/Artisan Street Dev"), Some(data))
    );
    assert_ne!(
        unit_name_for(&data.join("Artisan StreetX"), Some(data)),
        "artisan-forge-x.service"
    );
}

#[test]
fn units_live_in_the_xdg_config_directory() {
    let scratch = Path::new("/tmp/artisan-units");
    let (service, _) = dev_service(scratch);
    assert_eq!(
        service.unit_path(),
        scratch.join("config/systemd/user/artisan-forge-dev.service")
    );
    let home_only = UserDirectories {
        home: Some(scratch.join("home")),
        ..UserDirectories::default()
    };
    assert_eq!(
        home_only.unit_directory().expect("unit directory"),
        scratch.join("home/.config/systemd/user")
    );
    assert_eq!(
        home_only.data_directory(),
        Some(scratch.join("home/.local/share"))
    );
}

#[test]
fn the_unit_runs_the_permanent_ae_in_the_foreground_and_stops_with_sigint() {
    let scratch = tempfile::tempdir().expect("scratch");
    let (service, root) = dev_service(scratch.path());
    let unit = service
        .render(Some("/usr/bin:/home/ada/.nix-profile/bin"))
        .expect("render");
    let ae = root.join("bin/ae");
    assert!(unit.contains(&format!(
        "ExecStart=\"{}\" start --foreground\n",
        ae.display()
    )));
    assert!(unit.contains(&format!("X-ArtisanInstallRoot={}\n", root.display())));
    assert!(unit.contains("Environment=\"PATH=/usr/bin:/home/ada/.nix-profile/bin\"\n"));
    for line in [
        "Type=simple",
        "Restart=on-failure",
        "KillSignal=SIGINT",
        "UMask=0077",
        "WantedBy=default.target",
    ] {
        assert!(unit.lines().any(|candidate| candidate == line), "{line}");
    }
    assert!(
        !service
            .render(None)
            .expect("render")
            .contains("Environment=")
    );
}

#[test]
fn exec_arguments_escape_quotes_specifiers_and_variables() {
    assert_eq!(
        quote_exec_argument("/data/A \"q\" 100% $HOME\\x").expect("quote"),
        r#""/data/A \"q\" 100%% $$HOME\\x""#
    );
    assert!(quote_exec_argument("line\nbreak").is_err());
}

#[test]
fn install_writes_reloads_and_enables_then_is_idempotent() {
    let scratch = tempfile::tempdir().expect("scratch");
    let (service, _) = dev_service(scratch.path());
    let systemctl = FakeSystemctl::default();
    assert_eq!(service.inspect().expect("inspect"), UnitFile::Absent);

    assert!(
        service
            .install(&systemctl, Some("/usr/bin"))
            .expect("install")
    );
    assert_eq!(
        fs::read_to_string(service.unit_path()).expect("unit"),
        service.render(Some("/usr/bin")).expect("render")
    );
    assert_eq!(
        service.inspect().expect("inspect"),
        UnitFile::Owned { current: true }
    );
    assert_eq!(
        systemctl.calls.borrow().as_slice(),
        ["daemon-reload", "enable artisan-forge-dev.service"]
    );

    // An unchanged unit is not rewritten; a changed PATH is.
    assert!(
        !service
            .install(&systemctl, Some("/usr/bin"))
            .expect("again")
    );
    assert!(
        service
            .install(&systemctl, Some("/opt/bin"))
            .expect("new path")
    );
    assert!(
        fs::read_to_string(service.unit_path())
            .expect("unit")
            .contains("PATH=/opt/bin")
    );
}

#[test]
fn a_unit_for_another_executable_is_drifted_and_a_foreign_unit_is_refused() {
    let scratch = tempfile::tempdir().expect("scratch");
    let (service, root) = dev_service(scratch.path());
    let systemctl = FakeSystemctl::default();
    service.install(&systemctl, None).expect("install");
    let unit = fs::read_to_string(service.unit_path()).expect("unit");
    fs::write(
        service.unit_path(),
        unit.replace(&format!("{}/bin/ae", root.display()), "/nix/store/x/bin/ae"),
    )
    .expect("drift");
    assert_eq!(
        service.inspect().expect("inspect"),
        UnitFile::Owned { current: false }
    );
    // Configuring again repairs the drift.
    service.install(&systemctl, None).expect("repair");
    assert_eq!(
        service.inspect().expect("inspect"),
        UnitFile::Owned { current: true }
    );

    // A hand-written unit of the same name is never overwritten or removed.
    let hand_written = "[Service]\nExecStart=/nix/store/x/bin/forge-host\n";
    fs::write(service.unit_path(), hand_written).expect("foreign unit");
    assert_eq!(service.inspect().expect("inspect"), UnitFile::Foreign);
    assert!(service.install(&systemctl, None).is_err());
    assert!(service.remove(&systemctl).is_err());
    assert_eq!(
        fs::read_to_string(service.unit_path()).expect("unit"),
        hand_written
    );
}

#[test]
fn remove_disables_stops_and_deletes_only_an_owned_unit() {
    let scratch = tempfile::tempdir().expect("scratch");
    let (service, _) = dev_service(scratch.path());
    let systemctl = FakeSystemctl {
        enabled: true,
        active: true,
        ..FakeSystemctl::default()
    };
    service.remove(&systemctl).expect("absent is removed");
    assert!(systemctl.calls.borrow().is_empty());

    service.install(&systemctl, None).expect("install");
    assert!(service.is_enabled(&systemctl).expect("enabled"));
    assert!(service.is_active(&systemctl).expect("active"));
    systemctl.calls.borrow_mut().clear();
    service.remove(&systemctl).expect("remove");
    assert!(!service.unit_path().exists());
    assert_eq!(
        systemctl.calls.borrow().as_slice(),
        ["disable --now artisan-forge-dev.service", "daemon-reload"]
    );
}

#[test]
fn health_follows_the_unit_file_and_the_manager() {
    let scratch = tempfile::tempdir().expect("scratch");
    let (service, root) = dev_service(scratch.path());
    let systemctl = FakeSystemctl {
        enabled: true,
        active: false,
        ..FakeSystemctl::default()
    };
    assert_eq!(service.health(&systemctl), ServiceHealth::Absent);
    service.install(&systemctl, None).expect("install");
    assert_eq!(
        service.health(&systemctl),
        ServiceHealth::Installed {
            enabled: Some(true),
            active: Some(false)
        }
    );
    let unit = fs::read_to_string(service.unit_path()).expect("unit");
    fs::write(
        service.unit_path(),
        unit.replace(&root.display().to_string(), "/elsewhere"),
    )
    .expect("rewrite");
    assert_eq!(service.health(&systemctl), ServiceHealth::Foreign);
}

#[test]
fn manager_refusals_are_reported() {
    let scratch = tempfile::tempdir().expect("scratch");
    let (service, _) = dev_service(scratch.path());
    let systemctl = FakeSystemctl {
        fail: Some("enable"),
        ..FakeSystemctl::default()
    };
    let error = service.install(&systemctl, None).expect_err("enable fails");
    assert!(
        error
            .to_string()
            .contains("enable artisan-forge-dev.service")
    );
    let refusing = FakeSystemctl {
        fail: Some("start"),
        ..FakeSystemctl::default()
    };
    assert!(service.start(&refusing).is_err());
    assert!(service.try_restart(&refusing).is_ok());
}
