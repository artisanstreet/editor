#[cfg(unix)]
mod tests {
    use artisan_paths::App;
    use std::net::TcpStream;
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

    enum Listen {
        AnyPort,
        Reserved(PortReservation),
    }

    impl Listen {
        fn configured(&self) -> String {
            match self {
                Self::AnyPort => "127.0.0.1:0".to_owned(),
                Self::Reserved(port) => port.address().to_string(),
            }
        }
    }

    struct Sandbox {
        root: tempfile::TempDir,
        installed: PathBuf,
        listen: Listen,
    }

    impl Sandbox {
        fn new() -> Self {
            Self::with_previous(&forge_binary(), Listen::AnyPort)
        }

        fn released() -> Self {
            Self::with_previous(
                &released_forge(),
                Listen::Reserved(PortReservation::loopback()),
            )
        }

        fn with_previous(previous: &Path, listen: Listen) -> Self {
            let sandbox = Self::without_forge(listen);
            test_support::executable::install(previous, &sandbox.installed);
            sandbox
        }

        fn without_forge(listen: Listen) -> Self {
            let root = tempfile::Builder::new()
                .prefix("artisan-install-rollback ")
                .tempdir()
                .expect("create the sandbox");
            let app = App::Forge.dir_under(&artisan_street(root.path()));
            let data = app.join("data");
            std::fs::create_dir_all(&data).expect("create Forge's data folder");
            let installed = app.join(App::Forge.binary_name());
            std::fs::write(
                data.join("config.json"),
                format!(r#"{{"listen": "{}"}}"#, listen.configured()),
            )
            .expect("write Forge's config");
            script(&root.path().join("bin/systemctl"), NO_SYSTEMD);
            Self {
                root,
                installed,
                listen,
            }
        }

        fn command(&self, args: &[&str]) -> Command {
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
                .env_remove("DBUS_SESSION_BUS_ADDRESS");
            command
        }

        fn ae(&self, args: &[&str]) -> Output {
            self.command(args).output().expect("run ae")
        }

        fn install_lock(&self) -> std::fs::File {
            let street = artisan_street(self.root.path());
            let mut name = street
                .file_name()
                .expect("the install folder has a name")
                .to_owned();
            name.push(".install.lock");
            let path = street.with_file_name(name);
            std::fs::create_dir_all(path.parent().expect("the lock has a folder"))
                .expect("create the lock's folder");
            std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(path)
                .expect("open the install lock")
        }

        fn ae_behind(
            &self,
            installing: std::fs::File,
            args: &[&str],
            meanwhile: impl FnOnce(),
        ) -> (std::process::ExitStatus, String) {
            use std::io::BufRead as _;
            let mut child = self
                .command(args)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .expect("run ae");
            let stderr = child.stderr.take().expect("ae's stderr is piped");
            let (send, lines) = std::sync::mpsc::channel();
            let reader = std::thread::spawn(move || {
                for line in std::io::BufReader::new(stderr)
                    .lines()
                    .map_while(Result::ok)
                {
                    if send.send(line).is_err() {
                        break;
                    }
                }
            });
            let deadline = std::time::Instant::now() + test_support::wait::LIVENESS;
            let mut said = String::new();
            while !said.contains("Waiting for another installer to finish") {
                let left = deadline.saturating_duration_since(std::time::Instant::now());
                let Ok(line) = lines.recv_timeout(left) else {
                    drop(installing);
                    child.wait().expect("wait for ae");
                    panic!("ae never waited for the installer holding the lock: {said}");
                };
                said.push_str(&line);
                said.push('\n');
            }
            meanwhile();
            drop(installing);
            let status = child.wait().expect("wait for ae");
            reader.join().expect("read ae's output");
            said.extend(lines.try_iter().map(|line| line + "\n"));
            (status, said)
        }

        fn stage_forge(&self, forge: &Path) -> String {
            let stage = self.root.path().join("stage").join(std::env::consts::OS);
            std::fs::create_dir_all(&stage).expect("create the stage");
            test_support::executable::install(
                Path::new(env!("CARGO_BIN_EXE_ae")),
                &stage.join("ae"),
            );
            test_support::executable::install(forge, &stage.join(App::Forge.binary_name()));
            self.root
                .path()
                .join("stage")
                .to_string_lossy()
                .into_owned()
        }

        fn listening(&self) -> bool {
            let answers =
                |address| TcpStream::connect_timeout(&address, Duration::from_millis(250)).is_ok();
            match &self.listen {
                Listen::AnyPort => {
                    forge_http::upgrade::gateway(&self.installed.with_file_name("data"))
                        .expect("read the gateway Forge recorded")
                        .is_some_and(|gateway| answers(gateway.address))
                }
                Listen::Reserved(port) => answers(port.address()),
            }
        }

        fn start_installed(&self) {
            let data = self.installed.with_file_name("data");
            let mut command = Command::new("sh");
            command
                .args([
                    "-c",
                    r#""$0" --log "$1" </dev/null >/dev/null 2>&1 & echo $!"#,
                ])
                .arg(&self.installed)
                .arg(data.join("forge.log"));
            isolate(&mut command, self.root.path());
            let output = command.output().expect("start the installed Forge");
            let pid: u32 = String::from_utf8_lossy(&output.stdout)
                .trim()
                .parse()
                .expect("the shell names the installed Forge's process");
            let deadline = std::time::Instant::now() + test_support::wait::LIVENESS;
            while !self.listening() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "the installed Forge never listened"
                );
            }
            std::fs::write(data.join("forge.pid"), pid.to_string())
                .expect("record the released Forge as the earlier ae did");
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            drop(self.ae(&["forge", "stop"]));
        }
    }

    #[test]
    fn a_forge_update_that_cannot_start_puts_the_previous_forge_back_running() {
        let sandbox = Sandbox::new();
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

    #[test]
    fn the_earlier_install_is_removed_only_once_the_new_one_is_in_place() {
        let sandbox = Sandbox::new();
        let earlier = sandbox.root.path().join("data/artisan/bin/ae");
        script(&earlier, "#!/bin/sh\necho the earlier ae\n");
        let stage = sandbox.root.path().join("stage").join(std::env::consts::OS);
        std::fs::create_dir_all(&stage).expect("create the stage");
        test_support::executable::install(Path::new(env!("CARGO_BIN_EXE_ae")), &stage.join("ae"));
        script(&stage.join(App::Forge.binary_name()), BROKEN_FORGE);
        let install = || {
            sandbox.ae(&[
                "install",
                "--editor",
                "none",
                "--forge",
                "here",
                "--yes",
                "--from",
                &sandbox.root.path().join("stage").to_string_lossy(),
            ])
        };

        let failed = install();

        let said = format!(
            "{}{}",
            String::from_utf8_lossy(&failed.stdout),
            String::from_utf8_lossy(&failed.stderr)
        );
        assert!(!failed.status.success(), "the install fails: {said}");
        assert!(
            earlier.is_file(),
            "a failed install leaves the earlier one runnable: {said}"
        );

        test_support::executable::install(&forge_binary(), &stage.join(App::Forge.binary_name()));
        let installed = install();

        let said = format!(
            "{}{}",
            String::from_utf8_lossy(&installed.stdout),
            String::from_utf8_lossy(&installed.stderr)
        );
        assert!(installed.status.success(), "the install succeeds: {said}");
        assert!(
            !earlier.exists(),
            "the earlier install goes once the new one runs: {said}"
        );
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
        let sandbox = Sandbox::released();
        let data = App::Forge
            .dir_under(&artisan_street(sandbox.root.path()))
            .join("data");
        let database = data.join("forge.sqlite3");
        let mut released = Vec::new();
        flate2::read::GzDecoder::new(RELEASED_DATABASE)
            .read_to_end(&mut released)
            .expect("unpack the released database");
        std::fs::write(&database, released).expect("give the released Forge its database");
        sandbox.start_installed();
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
        let sandbox = Sandbox::released();
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
        let sandbox = Sandbox::released();
        let database = App::Forge
            .dir_under(&artisan_street(sandbox.root.path()))
            .join("data")
            .join("forge.sqlite3");
        let mut released = Vec::new();
        flate2::read::GzDecoder::new(RELEASED_DATABASE)
            .read_to_end(&mut released)
            .expect("unpack the released database");
        std::fs::write(&database, released).expect("give the released Forge its database");
        sandbox.start_installed();
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
        let sandbox = Sandbox::new();
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
        let sandbox = Sandbox::new();
        let forge = App::Forge.dir_under(&artisan_street(sandbox.root.path()));
        std::fs::rename(&sandbox.installed, forge.join(".old-0-forge"))
            .expect("the cut-short install had moved Forge aside");
        std::fs::write(
            forge.join(".install-journal"),
            r#"{"entries":[{"name":"forge","aside":".old-0-forge"}],"before":{"Forge":{"database":"Nothing"}}}"#,
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

    fn said(output: &Output) -> String {
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    }

    fn install_from_nowhere(sandbox: &Sandbox) -> Output {
        sandbox.ae(&[
            "install",
            "--editor",
            "none",
            "--forge",
            "keep",
            "--yes",
            "--from",
            &sandbox
                .root
                .path()
                .join("no release here")
                .to_string_lossy(),
        ])
    }

    fn cut_short_first_update(sandbox: &Sandbox) -> PathBuf {
        use std::io::Read as _;
        let forge = App::Forge.dir_under(&artisan_street(sandbox.root.path()));
        let database = forge.join("data").join("forge.sqlite3");
        let mut released = Vec::new();
        flate2::read::GzDecoder::new(RELEASED_DATABASE)
            .read_to_end(&mut released)
            .expect("unpack the released database");
        std::fs::write(&database, released).expect("give the released Forge its database");
        std::fs::write(
            forge.join(".install-journal"),
            format!(
                r#"{{"entries":[{{"name":"forge","aside":".old-0-forge"}}],"before":{{"Forge":{{"database":{{"Schema":{RELEASED_SCHEMA}}}}}}}}}"#
            ),
        )
        .expect("the cut-short install had recorded the released database");
        std::fs::rename(&sandbox.installed, forge.join(".old-0-forge"))
            .expect("the cut-short install had moved the released Forge aside");
        test_support::executable::install(&forge_binary(), &sandbox.installed);
        database
    }

    #[test]
    fn starting_forge_after_a_first_update_was_cut_short_puts_the_released_forge_back_first() {
        let sandbox = Sandbox::released();
        let database = cut_short_first_update(&sandbox);
        let forge = App::Forge.dir_under(&artisan_street(sandbox.root.path()));

        let started = sandbox.ae(&["forge", "start"]);

        let said = said(&started);
        assert!(started.status.success(), "{said}");
        assert!(
            said.contains("Put back the Forge files an interrupted install left half replaced"),
            "{said}"
        );
        assert_eq!(
            std::fs::read(&sandbox.installed).expect("read the installed Forge"),
            std::fs::read(released_forge()).expect("read the released Forge"),
            "the released Forge binary is back: {said}"
        );
        assert_eq!(
            schema_of(&database),
            RELEASED_SCHEMA,
            "the new Forge never upgraded the released database: {said}"
        );
        assert!(!forge.join(".install-journal").exists(), "{said}");
        assert!(
            sandbox.listening(),
            "the released Forge serves again: {said}"
        );
    }

    #[test]
    fn a_first_update_cut_short_after_the_new_forge_upgraded_the_database_is_rolled_back_by_the_next_ae()
     {
        let sandbox = Sandbox::released();
        let database = cut_short_first_update(&sandbox);
        let forge = App::Forge.dir_under(&artisan_street(sandbox.root.path()));
        let data = forge.join("data");
        sandbox.start_installed();
        assert_eq!(
            schema_of(&database),
            forge_store::Store::schema_version(),
            "the new Forge upgraded the database before the install was cut short"
        );

        let recovered = install_from_nowhere(&sandbox);

        let said = said(&recovered);
        assert!(
            !recovered.status.success(),
            "there is nothing to install from: {said}"
        );
        assert!(
            said.contains("Put back the Forge files an interrupted install left half replaced"),
            "{said}"
        );
        assert_eq!(
            std::fs::read(&sandbox.installed).expect("read the installed Forge"),
            std::fs::read(released_forge()).expect("read the released Forge"),
            "the released Forge binary is back: {said}"
        );
        assert_eq!(
            schema_of(&database),
            RELEASED_SCHEMA,
            "the released Forge has its own database again: {said}"
        );
        assert!(
            data.join(format!(
                "forge.sqlite3.rolled-back-from-{}-0",
                forge_store::Store::schema_version()
            ))
            .is_file(),
            "the upgraded database is kept beside it: {said}"
        );
        assert!(!forge.join(".install-journal").exists(), "{said}");
        assert!(
            sandbox.listening(),
            "the released Forge serves again: {said}"
        );
    }

    #[test]
    fn a_record_of_an_interrupted_install_that_cannot_be_read_says_how_out_and_never_blocks_uninstalling()
     {
        let sandbox = Sandbox::new();
        let forge = App::Forge.dir_under(&artisan_street(sandbox.root.path()));
        let journal = forge.join(".install-journal");
        std::fs::write(&journal, "{\"entries\":[").expect("leave a record that doesn't read");

        let blocked = install_from_nowhere(&sandbox);

        let told = said(&blocked);
        assert!(!blocked.status.success(), "{told}");
        assert!(told.contains(&journal.display().to_string()), "{told}");
        assert!(told.contains("ae uninstall forge"), "{told}");

        let removed = sandbox.ae(&["uninstall", "forge", "--yes"]);

        assert!(removed.status.success(), "{}", said(&removed));
        assert!(!sandbox.installed.exists(), "Forge was uninstalled");
        assert!(!journal.exists(), "the record went with it");
    }

    #[test]
    fn a_version_file_that_cannot_be_read_neither_blocks_repairing_nor_uninstalling_forge() {
        let sandbox = Sandbox::new();
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

    #[test]
    fn an_installer_waiting_since_before_an_uninstall_still_keeps_the_next_one_out() {
        let sandbox = Sandbox::new();
        let stage = sandbox.stage_forge(&forge_binary());
        let waiting = sandbox.install_lock();

        let removed = sandbox.ae(&["uninstall", "forge", "--yes"]);
        assert!(removed.status.success(), "{}", said(&removed));
        waiting
            .lock()
            .expect("the waiting installer gets the lock after the uninstall");
        let (installed, told) = sandbox.ae_behind(
            waiting,
            &[
                "install", "--editor", "none", "--forge", "here", "--yes", "--from", &stage,
            ],
            || {},
        );

        assert!(installed.success(), "{told}");
        assert!(sandbox.installed.is_file(), "Forge is installed: {told}");
    }

    #[test]
    fn an_install_waiting_on_one_that_dies_mid_swap_puts_its_files_back_before_installing() {
        let sandbox = Sandbox::new();
        let forge = App::Forge.dir_under(&artisan_street(sandbox.root.path()));
        let stage = sandbox.stage_forge(&forge_binary());
        let dying = sandbox.install_lock();
        dying.lock().expect("the dying installer holds the lock");

        let (installed, told) = sandbox.ae_behind(
            dying,
            &[
                "install", "--editor", "none", "--forge", "keep", "--yes", "--from", &stage,
            ],
            || {
                std::fs::rename(&sandbox.installed, forge.join(".old-0-forge"))
                    .expect("the dying installer moved Forge aside");
                std::fs::write(
                    forge.join(".install-journal"),
                    r#"{"entries":[{"name":"forge","aside":".old-0-forge"}],"before":{"Forge":{"database":"Nothing"}}}"#,
                )
                .expect("the dying installer recorded the move");
            },
        );

        assert!(installed.success(), "{told}");
        assert!(
            told.contains("Put back the Forge files an interrupted install left half replaced"),
            "{told}"
        );
        assert!(!forge.join(".install-journal").exists(), "{told}");
        assert!(sandbox.listening(), "the updated Forge serves: {told}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_first_forge_install_that_cannot_start_leaves_no_login_service_behind() {
        let sandbox = Sandbox::without_forge(Listen::AnyPort);
        script(
            &sandbox.root.path().join("bin/systemctl"),
            "#!/bin/sh\ncase \"$2\" in is-active) exit 3;; *) exit 0;; esac\n",
        );
        script(
            &sandbox.root.path().join("bin/journalctl"),
            "#!/bin/sh\nexit 0\n",
        );
        let stage = sandbox.stage_forge(&forge_binary());

        let failed = sandbox.ae(&[
            "install", "--editor", "none", "--forge", "here", "--yes", "--from", &stage,
        ]);

        let said = said(&failed);
        assert!(
            !failed.status.success(),
            "Forge never becomes active: {said}"
        );
        assert!(
            !sandbox.installed.exists(),
            "Forge's files are gone again: {said}"
        );
        let unit = sandbox
            .root
            .path()
            .join("config/systemd/user/artisan-forge.service");
        assert!(
            !unit.exists(),
            "no unit starts the missing Forge at login: {said}"
        );
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
        let sandbox = Sandbox::new();
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
        let sandbox = Sandbox::new();
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

    fn consent_to_replace(contents: &str) -> String {
        use sha2::Digest as _;
        format!("file:{}", hex::encode(sha2::Sha256::digest(contents)))
    }

    #[test]
    fn consent_handed_over_from_an_interactive_install_replaces_only_the_ae_it_was_given_for() {
        const THEIRS: &str = "#!/bin/sh\necho my own ae\n";
        let sandbox = Sandbox::new();
        let theirs = sandbox.root.path().join("home/.local/bin/ae");
        script(&theirs, THEIRS);
        let stage = ae_only_stage(&sandbox);
        let handed_over = |found: &str| {
            sandbox.ae(&[
                "install",
                "--apply",
                "--editor",
                "none",
                "--forge",
                "none",
                "--replace-ae",
                &theirs.to_string_lossy(),
                "--replace-ae-found",
                found,
                "--from",
                &stage,
            ])
        };

        let changed_since = handed_over(&consent_to_replace("#!/bin/sh\necho an older ae\n"));

        assert!(
            !changed_since.status.success(),
            "consent given for another file doesn't cover this one"
        );
        assert!(
            String::from_utf8_lossy(&changed_since.stderr).contains("--yes"),
            "{}",
            String::from_utf8_lossy(&changed_since.stderr)
        );
        assert_eq!(
            std::fs::read_to_string(&theirs).expect("their ae is still there"),
            THEIRS
        );

        let agreed = handed_over(&consent_to_replace(THEIRS));

        assert!(
            agreed.status.success(),
            "{}",
            String::from_utf8_lossy(&agreed.stderr)
        );
        assert_eq!(
            std::fs::read_link(&theirs).expect("ae is linked"),
            artisans_ae(&sandbox)
        );
    }

    #[test]
    fn a_failed_install_keeps_the_ae_that_isnt_artisans_and_a_successful_one_replaces_it() {
        let sandbox = Sandbox::new();
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
