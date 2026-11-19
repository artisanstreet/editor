#[cfg(unix)]
mod tests {
    use artisan_paths::App;
    use std::net::{SocketAddr, TcpStream};
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output};
    use std::time::Duration;
    use test_support::net::PortReservation;
    use test_support::sandbox::{artisan_street, isolate};

    const BROKEN_FORGE: &str = "#!/bin/sh\nif [ \"$1\" = \"--schema-version\" ]; then echo 999; exit 0; fi\necho 'this Forge cannot start' >&2\nexit 3\n";
    const NO_SYSTEMD: &str = "#!/bin/sh\nexit 1\n";

    fn forge_binary() -> PathBuf {
        let forge = Path::new(env!("CARGO_BIN_EXE_ae")).with_file_name("forge");
        assert!(
            forge.is_file(),
            "{} is missing; build it first (`cargo build -p forge`)",
            forge.display()
        );
        forge
    }

    fn script(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().expect("a script has a folder"))
            .expect("create the script folder");
        std::fs::write(path, text).expect("write the script");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
            .expect("make the script runnable");
    }

    struct Sandbox {
        root: tempfile::TempDir,
        installed: PathBuf,
        address: SocketAddr,
    }

    impl Sandbox {
        fn new(address: SocketAddr) -> Self {
            assert!(address.ip().is_loopback(), "{address} is not loopback");
            let root = tempfile::Builder::new()
                .prefix("artisan-install-rollback ")
                .tempdir()
                .expect("create the sandbox");
            let app = App::Forge.dir_under(&artisan_street(root.path()));
            let data = app.join("data");
            std::fs::create_dir_all(&data).expect("create Forge's data folder");
            let installed = app.join(App::Forge.binary_name());
            std::fs::copy(forge_binary(), &installed).expect("install the previous Forge");
            std::fs::write(
                data.join("config.json"),
                format!(r#"{{"listen": "{address}"}}"#),
            )
            .expect("write Forge's config");
            script(&root.path().join("bin/systemctl"), NO_SYSTEMD);
            Self {
                root,
                installed,
                address,
            }
        }

        fn ae(&self, args: &[&str]) -> Output {
            let mut command = Command::new(env!("CARGO_BIN_EXE_ae"));
            command.args(args);
            isolate(&mut command, self.root.path());
            let path = std::env::var_os("PATH").unwrap_or_default();
            let mut paths = vec![self.root.path().join("bin")];
            paths.extend(std::env::split_paths(&path));
            command
                .env(
                    "PATH",
                    std::env::join_paths(paths).expect("join the PATH entries"),
                )
                .env_remove("DBUS_SESSION_BUS_ADDRESS")
                .output()
                .expect("run ae")
        }

        fn listening(&self) -> bool {
            TcpStream::connect_timeout(&self.address, Duration::from_millis(250)).is_ok()
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            drop(self.ae(&["forge", "stop"]));
        }
    }

    #[test]
    fn a_forge_update_that_cannot_start_puts_the_previous_forge_back_running() {
        let port = PortReservation::loopback();
        let sandbox = Sandbox::new(port.address());
        let started = sandbox.ae(&["forge", "start"]);
        assert!(
            started.status.success(),
            "the previous Forge starts: {}",
            String::from_utf8_lossy(&started.stderr)
        );
        let stage = sandbox.root.path().join("stage").join(std::env::consts::OS);
        std::fs::create_dir_all(&stage).expect("create the stage");
        std::fs::copy(env!("CARGO_BIN_EXE_ae"), stage.join("ae")).expect("stage ae");
        script(&stage.join(App::Forge.binary_name()), BROKEN_FORGE);

        let update = sandbox.ae(&[
            "install",
            "--editor",
            "none",
            "--forge",
            "here",
            "--yes",
            "--from",
            &sandbox.root.path().join("stage").to_string_lossy(),
        ]);

        let said = format!(
            "{}{}",
            String::from_utf8_lossy(&update.stdout),
            String::from_utf8_lossy(&update.stderr)
        );
        assert!(!update.status.success(), "the update fails: {said}");
        assert!(
            said.contains("the previous Forge is back in place and running"),
            "{said}"
        );
        assert_eq!(
            std::fs::read(&sandbox.installed).expect("read the installed Forge"),
            std::fs::read(forge_binary()).expect("read the previous Forge"),
            "the previous Forge binary is back"
        );
        assert!(sandbox.listening(), "the previous Forge serves again");
    }

    fn ae_only_stage(sandbox: &Sandbox) -> String {
        let stage = sandbox.root.path().join("stage").join(std::env::consts::OS);
        std::fs::create_dir_all(&stage).expect("create the stage");
        std::fs::copy(env!("CARGO_BIN_EXE_ae"), stage.join("ae")).expect("stage ae");
        sandbox
            .root
            .path()
            .join("stage")
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn an_installer_run_for_another_one_never_deletes_an_ae_that_isnt_artisans() {
        let port = PortReservation::loopback();
        let sandbox = Sandbox::new(port.address());
        let theirs = sandbox.root.path().join("home/.local/bin/ae");
        script(&theirs, "#!/bin/sh\necho my own ae\n");
        let stage = ae_only_stage(&sandbox);

        let applied = sandbox.ae(&[
            "install", "--apply", "--editor", "none", "--forge", "none", "--from", &stage,
        ]);

        assert!(!applied.status.success(), "the install stops to ask");
        assert!(
            String::from_utf8_lossy(&applied.stderr).contains("--yes"),
            "{}",
            String::from_utf8_lossy(&applied.stderr)
        );
        assert_eq!(
            std::fs::read_to_string(&theirs).expect("their ae is still there"),
            "#!/bin/sh\necho my own ae\n"
        );

        let agreed = sandbox.ae(&[
            "install", "--apply", "--yes", "--editor", "none", "--forge", "none", "--from", &stage,
        ]);
        assert!(
            agreed.status.success(),
            "{}",
            String::from_utf8_lossy(&agreed.stderr)
        );
        assert!(
            theirs
                .symlink_metadata()
                .expect("ae is linked")
                .is_symlink(),
            "with --yes, Artisan's ae takes the place"
        );
    }

    fn artisans_ae(sandbox: &Sandbox) -> PathBuf {
        App::Ae
            .dir_under(&artisan_street(sandbox.root.path()))
            .join(App::Ae.binary_name())
    }

    fn install_ae_only(sandbox: &Sandbox, stage: &str, yes: bool) -> Output {
        let mut args = vec!["install", "--apply", "--editor", "none", "--forge", "none"];
        if yes {
            args.push("--yes");
        }
        args.extend(["--from", stage]);
        sandbox.ae(&args)
    }

    #[test]
    fn an_ae_link_from_a_dotfile_manager_is_only_replaced_with_consent() {
        let port = PortReservation::loopback();
        let sandbox = Sandbox::new(port.address());
        let dotfiles = sandbox.root.path().join("home/dotfiles/bin/ae");
        script(&dotfiles, "#!/bin/sh\necho my own ae\n");
        let link = sandbox.root.path().join("home/.local/bin/ae");
        std::fs::create_dir_all(link.parent().expect("the link has a folder"))
            .expect("create ~/.local/bin");
        std::os::unix::fs::symlink(&dotfiles, &link).expect("link the dotfile ae");
        let stage = ae_only_stage(&sandbox);

        let applied = install_ae_only(&sandbox, &stage, false);

        assert!(!applied.status.success(), "the install stops to ask");
        assert!(
            String::from_utf8_lossy(&applied.stderr).contains("--yes"),
            "{}",
            String::from_utf8_lossy(&applied.stderr)
        );
        assert_eq!(
            std::fs::read_link(&link).expect("their link is still there"),
            dotfiles
        );

        let agreed = install_ae_only(&sandbox, &stage, true);

        assert!(
            agreed.status.success(),
            "{}",
            String::from_utf8_lossy(&agreed.stderr)
        );
        assert_eq!(
            std::fs::read_link(&link).expect("ae is linked"),
            artisans_ae(&sandbox)
        );
        assert_eq!(
            std::fs::read_to_string(&dotfiles).expect("their ae itself is untouched"),
            "#!/bin/sh\necho my own ae\n"
        );
    }

    #[test]
    fn a_failed_install_keeps_the_ae_that_isnt_artisans_and_a_successful_one_replaces_it() {
        let port = PortReservation::loopback();
        let sandbox = Sandbox::new(port.address());
        let theirs = sandbox.root.path().join("home/.local/bin/ae");
        script(&theirs, "#!/bin/sh\necho my own ae\n");
        let blocker = App::Ae.dir_under(&artisan_street(sandbox.root.path()));
        std::fs::write(&blocker, "in the way of ae's folder").expect("block ae's folder");
        let stage = ae_only_stage(&sandbox);

        let failed = install_ae_only(&sandbox, &stage, true);

        assert!(!failed.status.success(), "the install fails");
        assert_eq!(
            std::fs::read_to_string(&theirs).expect("their ae is still there"),
            "#!/bin/sh\necho my own ae\n"
        );

        std::fs::remove_file(&blocker).expect("clear the way");
        let installed = install_ae_only(&sandbox, &stage, true);

        assert!(
            installed.status.success(),
            "{}",
            String::from_utf8_lossy(&installed.stderr)
        );
        assert_eq!(
            std::fs::read_link(&theirs).expect("ae is linked"),
            artisans_ae(&sandbox)
        );
    }
}
