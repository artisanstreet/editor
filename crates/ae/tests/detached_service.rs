//! `ae forge start|stop|restart` on a detached Forge, through the built
//! binaries, never an installed service. Every instance is its own Artisan
//! Street (HOME and XDG folders in a temp dir) with `forge` installed in it
//! and its own loopback port.

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output};
    use std::time::Duration;

    /// The `forge` built beside this `ae` (`cargo test --workspace` builds it).
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
            let app = root.path().join("data/artisan-street/forge");
            let data = app.join("data");
            std::fs::create_dir_all(&data).unwrap();
            std::fs::copy(forge_binary(), app.join("forge")).unwrap();
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
            command
                .args(["forge", action])
                .env("HOME", self.root.path().join("home"))
                .env("XDG_DATA_HOME", self.root.path().join("data"))
                // An isolated config folder has no systemd unit, so the
                // detached branch runs. No installed service is touched.
                .env("XDG_CONFIG_HOME", self.root.path().join("config"));
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
                // Stop signals a pid only when it runs this instance's own
                // `forge`, so this can't reach the developer's Forge.
                self.command("stop").output().ok();
            }
        }
    }

    fn unused_loopback_address() -> SocketAddr {
        let reservation = TcpListener::bind("127.0.0.1:0").unwrap();
        reservation.local_addr().unwrap()
    }

    fn stderr(output: &Output) -> String {
        String::from_utf8_lossy(&output.stderr).into_owned()
    }

    #[test]
    fn restart_waits_for_its_own_shutdown_and_new_listener() {
        let instance = Instance::new(unused_loopback_address());
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

        // A pid file in another install doesn't let that install stop this
        // Forge: the process runs a different `forge`.
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
        let instance = Instance::new(unused_loopback_address());
        // Holding this instance's data lock makes its Forge exit before it
        // binds, a real startup failure that touches no other Forge.
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
        let instance = Instance::new(unused_loopback_address());
        std::fs::create_dir(instance.data.join("forge.sqlite3")).unwrap();
        let output = instance.run("start");
        assert!(!output.status.success());
        let error = stderr(&output);
        assert!(error.contains("exited while starting"), "{error}");
        assert!(error.contains("open the store"), "{error}");
        assert!(!instance.listening());
    }
}
