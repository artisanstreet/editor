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

    const BROKEN_FORGE: &str = "#!/bin/sh\nif [ \"$1\" = \"--schema-version\" ]; then echo '{\"schema\":999,\"recall_index\":1}'; exit 0; fi\necho 'this Forge cannot start' >&2\nexit 3\n";
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
            Self::with_previous(address, &forge_binary())
        }

        fn with_previous(address: SocketAddr, previous: &Path) -> Self {
            assert!(address.ip().is_loopback(), "{address} is not loopback");
            let root = tempfile::Builder::new()
                .prefix("artisan-install-rollback ")
                .tempdir()
                .expect("create the sandbox");
            let app = App::Forge.dir_under(&artisan_street(root.path()));
            let data = app.join("data");
            std::fs::create_dir_all(&data).expect("create Forge's data folder");
            let installed = app.join(App::Forge.binary_name());
            test_support::executable::install(previous, &installed);
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
        test_support::executable::install(Path::new(env!("CARGO_BIN_EXE_ae")), &stage.join("ae"));
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

    const RELEASED_SCHEMA: usize = 26;
    const RELEASED_DATABASE: &[u8] =
        include_bytes!("../../forge-store/tests/fixtures/released/schema-26.sqlite3.gz");

    fn released_forge() -> PathBuf {
        let legacy = Path::new(env!("CARGO_BIN_EXE_ae"))
            .with_file_name("examples")
            .join("legacy_forge");
        assert!(
            legacy.is_file(),
            "{} is missing; build it first (`cargo build -p ae --examples`)",
            legacy.display()
        );
        legacy
    }

    fn schema_of(database: &Path) -> usize {
        rusqlite::Connection::open(database)
            .expect("open Forge's database")
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("read the schema version")
    }

    #[test]
    fn a_first_update_that_fails_after_upgrading_the_database_puts_the_released_forge_back_running()
    {
        use std::io::Read as _;
        let port = PortReservation::loopback();
        let sandbox = Sandbox::with_previous(port.address(), &released_forge());
        let data = App::Forge
            .dir_under(&artisan_street(sandbox.root.path()))
            .join("data");
        let database = data.join("forge.sqlite3");
        let mut released = Vec::new();
        flate2::read::GzDecoder::new(RELEASED_DATABASE)
            .read_to_end(&mut released)
            .expect("unpack the released database");
        std::fs::write(&database, released).expect("give the released Forge its database");
        let started = sandbox.ae(&["forge", "start"]);
        assert!(
            started.status.success(),
            "the released Forge starts: {}",
            String::from_utf8_lossy(&started.stderr)
        );
        let stage = sandbox.root.path().join("stage").join(std::env::consts::OS);
        std::fs::create_dir_all(&stage).expect("create the stage");
        test_support::executable::install(Path::new(env!("CARGO_BIN_EXE_ae")), &stage.join("ae"));
        script(
            &stage.join(App::Forge.binary_name()),
            &format!(
                "#!/bin/sh\nexec '{}' --listen 192.0.2.1:9 \"$@\"\n",
                forge_binary().display()
            ),
        );

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
            schema_of(&database),
            RELEASED_SCHEMA,
            "the released Forge has its own database again"
        );
        assert!(
            data.join(format!(
                "forge.sqlite3.rolled-back-from-{}-0",
                forge_store::Store::schema_version()
            ))
            .is_file(),
            "the upgraded database is kept beside it"
        );
        assert!(sandbox.listening(), "the released Forge serves again");
    }

    fn a_failed_first_start_puts_the_released_forge_back_without_the_new_database(
        before: Option<&[u8]>,
    ) {
        let port = PortReservation::loopback();
        let sandbox = Sandbox::with_previous(port.address(), &released_forge());
        let data = App::Forge
            .dir_under(&artisan_street(sandbox.root.path()))
            .join("data");
        let database = data.join("forge.sqlite3");
        if let Some(before) = before {
            std::fs::write(&database, before).expect("leave the database the release had");
        }
        let stage = sandbox.root.path().join("stage").join(std::env::consts::OS);
        std::fs::create_dir_all(&stage).expect("create the stage");
        test_support::executable::install(Path::new(env!("CARGO_BIN_EXE_ae")), &stage.join("ae"));
        script(
            &stage.join(App::Forge.binary_name()),
            &format!(
                "#!/bin/sh\nexec '{}' --listen 192.0.2.1:9 \"$@\"\n",
                forge_binary().display()
            ),
        );

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
        assert!(
            data.join(format!(
                "forge.sqlite3.rolled-back-from-{}-0",
                forge_store::Store::schema_version()
            ))
            .is_file(),
            "the database the failed Forge made is kept aside: {said}"
        );
        assert!(
            !database.exists(),
            "the released Forge starts without a database, as it had none: {said}"
        );
        assert!(sandbox.listening(), "the released Forge serves again");
    }

    #[test]
    fn a_failed_update_of_a_forge_that_never_made_a_database_starts_it_without_the_new_one() {
        a_failed_first_start_puts_the_released_forge_back_without_the_new_database(None);
    }

    #[test]
    fn a_failed_update_of_a_forge_with_an_empty_database_starts_it_without_the_new_one() {
        a_failed_first_start_puts_the_released_forge_back_without_the_new_database(Some(b""));
    }

    #[test]
    fn an_update_that_cannot_read_the_database_version_leaves_the_released_forge_running() {
        use std::io::Read as _;
        let port = PortReservation::loopback();
        let sandbox = Sandbox::with_previous(port.address(), &released_forge());
        let database = App::Forge
            .dir_under(&artisan_street(sandbox.root.path()))
            .join("data")
            .join("forge.sqlite3");
        let mut released = Vec::new();
        flate2::read::GzDecoder::new(RELEASED_DATABASE)
            .read_to_end(&mut released)
            .expect("unpack the released database");
        std::fs::write(&database, released).expect("give the released Forge its database");
        let started = sandbox.ae(&["forge", "start"]);
        assert!(
            started.status.success(),
            "the released Forge starts: {}",
            String::from_utf8_lossy(&started.stderr)
        );
        std::fs::write(&database, "not a database").expect("damage the database file");
        let stage = sandbox.root.path().join("stage").join(std::env::consts::OS);
        std::fs::create_dir_all(&stage).expect("create the stage");
        test_support::executable::install(Path::new(env!("CARGO_BIN_EXE_ae")), &stage.join("ae"));
        test_support::executable::install(&forge_binary(), &stage.join(App::Forge.binary_name()));

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
        assert!(said.contains("storage version"), "{said}");
        assert!(
            sandbox.listening(),
            "the released Forge still serves: {said}"
        );
    }

    #[test]
    fn an_update_that_keeps_forge_updates_one_whose_version_file_is_missing() {
        let port = PortReservation::loopback();
        let sandbox = Sandbox::new(port.address());
        let forge = App::Forge.dir_under(&artisan_street(sandbox.root.path()));
        assert!(
            !forge.join("version").exists(),
            "an interrupted install left Forge without its version file"
        );
        let stage = sandbox.root.path().join("stage").join(std::env::consts::OS);
        std::fs::create_dir_all(&stage).expect("create the stage");
        test_support::executable::install(Path::new(env!("CARGO_BIN_EXE_ae")), &stage.join("ae"));
        test_support::executable::install(&forge_binary(), &stage.join(App::Forge.binary_name()));

        let update = sandbox.ae(&[
            "install",
            "--editor",
            "none",
            "--forge",
            "keep",
            "--yes",
            "--from",
            &sandbox.root.path().join("stage").to_string_lossy(),
        ]);

        let said = format!(
            "{}{}",
            String::from_utf8_lossy(&update.stdout),
            String::from_utf8_lossy(&update.stderr)
        );
        assert!(update.status.success(), "the update succeeds: {said}");
        assert!(
            forge.join("version").is_file(),
            "the installed Forge was updated too: {said}"
        );
        assert!(sandbox.listening(), "the updated Forge serves: {said}");
    }

    #[test]
    fn an_update_after_an_install_cut_short_between_its_renames_still_updates_forge() {
        let port = PortReservation::loopback();
        let sandbox = Sandbox::new(port.address());
        let forge = App::Forge.dir_under(&artisan_street(sandbox.root.path()));
        std::fs::rename(&sandbox.installed, forge.join(".old-0-forge"))
            .expect("the cut-short install had moved Forge aside");
        std::fs::write(
            forge.join(".install-journal"),
            r#"{"entries":[{"name":"forge","aside":".old-0-forge"}]}"#,
        )
        .expect("the cut-short install had recorded the move");
        let stage = sandbox.root.path().join("stage").join(std::env::consts::OS);
        std::fs::create_dir_all(&stage).expect("create the stage");
        test_support::executable::install(Path::new(env!("CARGO_BIN_EXE_ae")), &stage.join("ae"));
        test_support::executable::install(&forge_binary(), &stage.join(App::Forge.binary_name()));

        let update = sandbox.ae(&[
            "install",
            "--editor",
            "none",
            "--forge",
            "keep",
            "--yes",
            "--from",
            &sandbox.root.path().join("stage").to_string_lossy(),
        ]);

        let said = format!(
            "{}{}",
            String::from_utf8_lossy(&update.stdout),
            String::from_utf8_lossy(&update.stderr)
        );
        assert!(update.status.success(), "the update succeeds: {said}");
        assert!(
            said.contains("Put back the Forge files an interrupted install left half replaced"),
            "{said}"
        );
        assert!(
            forge.join("version").is_file(),
            "Forge was updated, not dropped from the update: {said}"
        );
        assert!(!forge.join(".install-journal").exists());
        assert!(sandbox.listening(), "the updated Forge serves: {said}");
    }

    #[test]
    fn a_version_file_that_cannot_be_read_neither_blocks_repairing_nor_uninstalling_forge() {
        let port = PortReservation::loopback();
        let sandbox = Sandbox::new(port.address());
        let forge = App::Forge.dir_under(&artisan_street(sandbox.root.path()));
        let version = forge.join("version");
        std::fs::create_dir(&version).expect("leave a version that doesn't read as a file");
        let stage = sandbox.root.path().join("stage").join(std::env::consts::OS);
        std::fs::create_dir_all(&stage).expect("create the stage");
        test_support::executable::install(Path::new(env!("CARGO_BIN_EXE_ae")), &stage.join("ae"));
        test_support::executable::install(&forge_binary(), &stage.join(App::Forge.binary_name()));

        let repaired = sandbox.ae(&[
            "install",
            "--editor",
            "none",
            "--forge",
            "keep",
            "--yes",
            "--from",
            &sandbox.root.path().join("stage").to_string_lossy(),
        ]);

        assert!(
            repaired.status.success(),
            "{}",
            String::from_utf8_lossy(&repaired.stderr)
        );
        assert!(version.is_file(), "the repair wrote Forge's version");

        std::fs::remove_file(&version).expect("drop the repaired version");
        std::fs::create_dir(&version).expect("leave a version that doesn't read as a file");
        let removed = sandbox.ae(&["uninstall", "forge", "--yes"]);

        assert!(
            removed.status.success(),
            "{}",
            String::from_utf8_lossy(&removed.stderr)
        );
        assert!(!sandbox.installed.exists(), "Forge was uninstalled");
    }

    fn ae_only_stage(sandbox: &Sandbox) -> String {
        let stage = sandbox.root.path().join("stage").join(std::env::consts::OS);
        std::fs::create_dir_all(&stage).expect("create the stage");
        test_support::executable::install(Path::new(env!("CARGO_BIN_EXE_ae")), &stage.join("ae"));
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
