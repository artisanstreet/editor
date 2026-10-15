//! The detached lifecycle through the built test binary, never an installed
//! service. Every child has its own temporary instance and loopback port.

#[cfg(unix)]
mod tests {
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::path::PathBuf;
    use std::process::{Command, Output};
    use std::time::Duration;

    struct Instance {
        root: tempfile::TempDir,
        data: PathBuf,
        address: SocketAddr,
    }

    impl Instance {
        fn new(address: SocketAddr) -> Self {
            assert!(address.ip().is_loopback());
            assert_ne!(address.port(), 0);
            let root = tempfile::Builder::new()
                .prefix("artisan-detached-service ")
                .tempdir()
                .unwrap();
            let data = root.path().join("forge instance");
            std::fs::create_dir_all(&data).unwrap();
            Self {
                root,
                data,
                address,
            }
        }

        fn command(&self, action: &str) -> Command {
            let mut command = Command::new(env!("CARGO_BIN_EXE_ae"));
            command
                .arg("--data-dir")
                .arg(&self.data)
                .args(["forge", action])
                .env("ARTISAN_FORGE_DATA", &self.data)
                .env("ARTISAN_FORGE_LISTEN", self.address.to_string())
                // An isolated config directory makes the systemd branch
                // impossible. No installed unit or service is modified.
                .env("XDG_CONFIG_HOME", self.root.path().join("config"))
                .env("XDG_DATA_HOME", self.root.path().join("data"));
            command
        }

        fn run(&self, action: &str) -> Output {
            self.command(action).output().unwrap()
        }

        fn succeeds(&self, action: &str) -> Output {
            let output = self.run(action);
            assert!(
                output.status.success(),
                "isolated {action} failed: {}",
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
                // Production stop verifies executable, forge/serve role and
                // this exact temporary data directory before signaling a PID.
                // This guard cannot target the developer's active instance.
                let _ = self.command("stop").output();
            }
        }
    }

    fn unused_loopback_address() -> SocketAddr {
        let reservation = TcpListener::bind("127.0.0.1:0").unwrap();
        reservation.local_addr().unwrap()
    }

    #[test]
    fn detached_restart_waits_for_its_own_shutdown_and_new_listener() {
        let instance = Instance::new(unused_loopback_address());
        instance.succeeds("start");
        assert!(
            instance.listening(),
            "start returns after its explicit listener is ready"
        );
        let first = instance.pid();
        let already = instance.succeeds("start");
        assert!(String::from_utf8_lossy(&already.stdout).contains("already running"));
        assert_eq!(
            instance.pid(),
            first,
            "idempotent start must not spawn another Forge"
        );

        instance.succeeds("restart");
        let second = instance.pid();
        assert_ne!(first, second, "restart creates a new owned process");
        assert!(
            instance.listening(),
            "restart returns after the replacement listener is ready"
        );

        // A stale pid file in another instance must not authorize stopping
        // this fixture's running child, even though its executable and role match.
        let other = Instance::new(instance.address);
        std::fs::write(other.data.join("forge.pid"), second.to_string()).unwrap();
        let refused = other.succeeds("stop");
        assert!(String::from_utf8_lossy(&refused.stdout).contains("not running"));
        assert!(
            instance.listening(),
            "a different instance cannot stop this child"
        );
        std::fs::remove_file(other.data.join("forge.pid")).unwrap();

        instance.succeeds("stop");
        assert!(
            !instance.listening(),
            "stop waits for this listener to close"
        );
        assert!(!instance.data.join("forge.pid").exists());
        let _released = forge_http::lock_data_dir(&instance.data)
            .expect("stop returns only after the owned process releases its data lock");
    }

    #[test]
    fn detached_start_rejects_an_occupied_listener_without_spawning() {
        let occupied = TcpListener::bind("127.0.0.1:0").unwrap();
        let instance = Instance::new(occupied.local_addr().unwrap());
        let output = instance.run("start");
        assert!(!output.status.success());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains("already occupied"), "{error}");
        assert!(error.contains(&instance.address.to_string()), "{error}");
        assert!(!instance.data.join("forge.pid").exists());
        assert!(!instance.data.join("forge.log").exists());
        assert!(
            instance.listening(),
            "the unrelated fixture listener remains untouched"
        );
    }

    #[test]
    fn detached_start_reports_early_child_exit_before_readiness() {
        let instance = Instance::new(unused_loopback_address());
        // Hold only this new fixture's lock. Its child fails before binding,
        // exercising real startup failure without touching an existing Forge.
        let _lock = forge_http::lock_data_dir(&instance.data).unwrap();
        let output = instance.run("start");
        assert!(!output.status.success());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains("exited while starting"), "{error}");
        assert!(!instance.listening());
        let log = std::fs::read_to_string(instance.data.join("forge.log")).unwrap();
        assert!(log.contains("another Forge is using"), "{log}");
    }

    #[test]
    fn detached_start_never_reports_ready_when_mandatory_store_setup_fails() {
        let instance = Instance::new(unused_loopback_address());
        std::fs::create_dir(instance.data.join("forge.sqlite3")).unwrap();
        let output = instance.run("start");
        assert!(!output.status.success());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains("exited while starting"), "{error}");
        assert!(!instance.listening());
        let log = std::fs::read_to_string(instance.data.join("forge.log")).unwrap();
        assert!(log.contains("open the store"), "{log}");
    }
}
