#[cfg(test)]
#[cfg(unix)]
mod tests {
    use artisan_paths::App;
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output};
    use std::time::Duration;
    use test_support::net::PortReservation;
    use test_support::sandbox::{artisan_street, isolate};

    fn forge_binary() -> PathBuf {
        let forge = Path::new(env!("CARGO_BIN_EXE_ae")).with_file_name("forge");
        assert!(
            forge.is_file(),
            "{} is missing; build it first (`cargo build -p forge`)",
            forge.display()
        );
        forge
    }

    struct Instance {
        root: tempfile::TempDir,
        data: PathBuf,
        address: SocketAddr,
    }

    impl Instance {
        fn new(address: SocketAddr) -> Self {
            assert!(address.ip().is_loopback(), "{address} is not loopback");
            assert_ne!(address.port(), 0, "the instance needs a fixed port");

            let root = tempfile::Builder::new()
                .prefix("artisan-detached-service ")
                .tempdir()
                .unwrap();
            let app = App::Forge.dir_under(&artisan_street(root.path()));
            let data = app.join("data");
            std::fs::create_dir_all(&data).unwrap();
            std::fs::copy(forge_binary(), app.join(App::Forge.binary_name())).unwrap();
            std::fs::write(
                data.join("config.json"),
                format!(r#"{{"listen": "{address}"}}"#),
            )
            .unwrap();
            Self {
                root,
                data,
                address,
            }
        }

        fn command(&self, action: &str) -> Command {
            let mut command = Command::new(env!("CARGO_BIN_EXE_ae"));
            command.args(["forge", action]);
            isolate(&mut command, self.root.path());
            command
        }

        fn run(&self, action: &str) -> Output {
            self.command(action).output().unwrap()
        }

        fn succeeds(&self, action: &str) -> Output {
            let output = self.run(action);
            assert!(
                output.status.success(),
                "{action} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            output
        }

        fn pid(&self) -> u32 {
            std::fs::read_to_string(self.data.join("forge.pid"))
                .unwrap()
                .trim()
                .parse()
                .unwrap()
        }

        fn listening(&self) -> bool {
            TcpStream::connect_timeout(&self.address, Duration::from_millis(250)).is_ok()
        }
    }

    impl Drop for Instance {
        fn drop(&mut self) {
            if self.data.join("forge.pid").exists() {
                let _stopped = self.command("stop").output();
            }
        }
    }

    fn stderr(output: &Output) -> String {
        String::from_utf8_lossy(&output.stderr).into_owned()
    }

    #[test]
    fn restart_waits_for_its_own_shutdown_and_new_listener() {
        let port = PortReservation::loopback();
        let instance = Instance::new(port.address());
        instance.succeeds("start");
        assert!(instance.listening(), "start returns once Forge answers");
        let first = instance.pid();
        let again = instance.succeeds("start");
        assert!(
            stderr(&again).contains("already running"),
            "{}",
            stderr(&again)
        );
        assert_eq!(instance.pid(), first, "a second start spawns nothing");

        instance.succeeds("restart");
        let second = instance.pid();
        assert_ne!(first, second, "restart starts a new Forge");
        assert!(instance.listening(), "restart returns once it answers");

        let other = Instance::new(instance.address);
        std::fs::write(other.data.join("forge.pid"), second.to_string()).unwrap();
        let refused = other.succeeds("stop");
        assert!(
            stderr(&refused).contains("wasn't running"),
            "{}",
            stderr(&refused)
        );
        assert!(instance.listening(), "another install can't stop it");
        std::fs::remove_file(other.data.join("forge.pid")).unwrap();

        instance.succeeds("stop");
        assert!(
            !instance.listening(),
            "stop waits for the listener to close"
        );
        assert!(!instance.data.join("forge.pid").exists());
        let _released = forge_http::lock_data_dir(&instance.data)
            .expect("stop returns only after Forge released its data lock");
    }

    #[test]
    fn service_operations_wait_for_the_existing_lifecycle_owner() {
        let port = PortReservation::loopback();
        let instance = Instance::new(port.address());
        for action in ["start", "restart", "stop"] {
            let first = std::fs::read(instance.data.join("forge.pid")).ok();
            let lock = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(instance.data.join("service.lock"))
                .unwrap();
            lock.lock().unwrap();
            let mut command = instance.command(action);
            let mut child = command
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            std::thread::sleep(Duration::from_millis(250));
            let blocked = child.try_wait().unwrap().is_none();
            let unchanged = std::fs::read(instance.data.join("forge.pid")).ok() == first
                && instance.listening() == first.is_some();
            drop(lock);
            let output = child.wait_with_output().unwrap();
            assert!(blocked, "{action} must wait for the lifecycle owner");
            assert!(
                unchanged,
                "{action} must preserve the running instance while waiting"
            );
            assert!(output.status.success(), "{action}: {}", stderr(&output));
        }
        assert!(
            !instance.listening(),
            "the serialized stop closes the listener"
        );
        assert!(
            !instance.data.join("forge.pid").exists(),
            "the serialized stop removes its PID"
        );
        let _released = forge_http::lock_data_dir(&instance.data).unwrap();
    }

    #[test]
    fn start_refuses_an_occupied_port_without_starting_forge() {
        let occupied = TcpListener::bind("127.0.0.1:0").unwrap();
        let instance = Instance::new(occupied.local_addr().unwrap());
        let output = instance.run("start");
        assert!(!output.status.success());
        let error = stderr(&output);
        assert!(error.contains("already occupied"), "{error}");
        assert!(error.contains(&instance.address.to_string()), "{error}");
        assert!(!instance.data.join("forge.pid").exists());
        assert!(!instance.data.join("forge.log").exists());
        assert!(instance.listening(), "the other listener is untouched");
    }

    #[test]
    fn start_reports_a_forge_that_exits_and_shows_its_log() {
        let port = PortReservation::loopback();
        let instance = Instance::new(port.address());
        let _lock = forge_http::lock_data_dir(&instance.data).unwrap();
        let output = instance.run("start");
        assert!(!output.status.success());
        let error = stderr(&output);
        assert!(error.contains("exited while starting"), "{error}");
        assert!(
            error.contains("another Forge is using"),
            "the log's last lines are shown: {error}"
        );
        assert!(!instance.listening());
    }

    #[test]
    fn start_never_reports_ready_when_the_store_cant_open() {
        let port = PortReservation::loopback();
        let instance = Instance::new(port.address());
        std::fs::create_dir(instance.data.join("forge.sqlite3")).unwrap();
        let output = instance.run("start");
        assert!(!output.status.success());
        let error = stderr(&output);
        assert!(error.contains("exited while starting"), "{error}");
        assert!(error.contains("private database"), "{error}");
        assert!(!instance.listening());
    }
}
