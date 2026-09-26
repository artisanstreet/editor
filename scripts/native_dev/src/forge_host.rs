//! Foreground Forge supervisor for a bare Forge home, used by the
//! remote-host integration check (`scripts/test_remote_host.py`).
//!
//! Installed Forges run under `ae start --foreground` (the Linux service);
//! this fixture drives the same pieces for a home without an installation:
//! stale readiness reconciliation and host invitation publishing come from
//! the product's `ae` library.
use artisan_editor_cli::{
    credentials,
    host_access::{self, HostAccess, ListenAddress},
    process::{self, ForgeReadiness},
};
use native_dev::provision;
use std::{
    collections::BTreeMap,
    net::SocketAddr,
    path::PathBuf,
    process::{Command, ExitCode},
    time::{Duration, Instant},
};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Forge host: {}", error_chain(error.as_ref()));
            ExitCode::FAILURE
        }
    }
}

/// Renders `error` and every source beneath it as `top: cause: cause`.
fn error_chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut rendered = error.to_string();
    let mut current = error.source();
    while let Some(source) = current {
        let message = source.to_string();
        if !rendered.ends_with(&message) {
            rendered.push_str(": ");
            rendered.push_str(&message);
        }
        current = source.source();
    }
    rendered
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mut options = BTreeMap::new();
    while let Some(key) = args.next() {
        if !["--home", "--forge", "--listen", "--advertise", "--name"].contains(&key.as_str()) {
            return Err("usage: forge-host --home PATH --forge PATH --listen IP:PORT --advertise IP:PORT --name NAME".into());
        }
        let value = args.next().ok_or("missing option value")?;
        if options.insert(key, value).is_some() {
            return Err("duplicate option".into());
        }
    }
    let get = |key: &str| options.get(key).cloned().ok_or("missing required option");
    let home = PathBuf::from(get("--home")?);
    let forge = PathBuf::from(get("--forge")?);
    let listen = resolve(&get("--listen")?)?;
    let advertise = resolve(&get("--advertise")?)?;
    if !home.is_absolute()
        || !forge.is_absolute()
        || listen.port() == 0
        || listen.port() != advertise.port()
    {
        return Err(
            "absolute paths and matching nonzero listening/advertised ports required".into(),
        );
    }
    let access = HostAccess::new(ListenAddress::Explicit(advertise), get("--name")?)?;
    let paths = credentials::provision_or_load(&home)?;
    let ready = home.join("readiness.json");
    // Serialize supervisors before checking custody or replacing crash leftovers.
    let _supervisor = lock_supervisor(&home)?;
    process::reconcile_stale_readiness(&ready, &home.join("custody"), &forge)?;
    let mut command = Command::new(&forge);
    for (key, value) in [
        ("--database", home.join("forge.db")),
        ("--custody", home.join("custody")),
        ("--ready-file", ready.clone()),
        ("--certificate-der", paths.certificate_paths()[0].clone()),
        ("--private-key-der", paths.private_key_path().to_path_buf()),
        (
            "--bootstrap-capability",
            paths.capability_path().to_path_buf(),
        ),
    ] {
        command.arg(key).arg(value);
    }
    command.arg("--listen").arg(listen.to_string());
    apply_policy(&mut command);
    wait_for_previous(&home.join("custody"))?;
    let mut child = command.spawn()?;
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let start = Instant::now();
        let readiness = loop {
            if let Some(readiness) = std::fs::read(&ready)
                .ok()
                .and_then(|bytes| ForgeReadiness::from_json(&bytes).ok())
                .filter(|readiness| readiness.pid() == child.id())
            {
                break readiness;
            }
            if child.try_wait()?.is_some() {
                return Err("Forge exited before becoming ready".into());
            }
            if start.elapsed() > Duration::from_secs(30) {
                return Err("Forge readiness timeout".into());
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        let invitation = host_access::publish_invitation(&home, &access, &readiness)?;
        eprintln!(
            "Forge ready at {advertise}; private invitation: {}",
            invitation.display()
        );
        let status = child.wait()?;
        if !status.success() {
            // The Forge prints its own complete error chain to the shared
            // stderr; the status keeps the exit-code classification visible.
            return Err(format!("Forge exited unsuccessfully ({status})").into());
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

fn resolve(value: &str) -> Result<SocketAddr, Box<dyn std::error::Error>> {
    let address: ListenAddress = value.parse()?;
    Ok(address.resolve()?)
}

fn apply_policy(command: &mut Command) {
    for (key, value) in [
        (
            "--admission-timeout-ms",
            provision::DEV_ADMISSION_TIMEOUT_MS,
        ),
        (
            "--handshake-timeout-ms",
            provision::DEV_HANDSHAKE_TIMEOUT_MS,
        ),
        ("--request-timeout-ms", provision::DEV_REQUEST_TIMEOUT_MS),
        ("--drain-timeout-ms", provision::DEV_DRAIN_TIMEOUT_MS),
        (
            "--admission-capacity",
            u64::from(provision::DEV_ADMISSION_CAPACITY),
        ),
        (
            "--requests-per-connection",
            u64::from(provision::DEV_REQUESTS_PER_CONNECTION),
        ),
        (
            "--native-run-claim-lease-ms",
            provision::DEV_RUN_CLAIM_LEASE_MS,
        ),
        (
            "--native-run-poll-interval-ms",
            provision::DEV_RUN_POLL_INTERVAL_MS,
        ),
        (
            "--native-run-retry-backoff-ms",
            provision::DEV_RUN_RETRY_BACKOFF_MS,
        ),
        (
            "--native-run-shutdown-budget-ms",
            provision::DEV_RUN_SHUTDOWN_BUDGET_MS,
        ),
        (
            "--native-run-queue-capacity",
            u64::from(provision::DEV_RUN_QUEUE_CAPACITY),
        ),
        (
            "--native-run-max-command-retries",
            u64::from(provision::DEV_RUN_MAX_COMMAND_RETRIES),
        ),
        ("--native-run-stream-after", provision::DEV_RUN_STREAM_AFTER),
    ] {
        command.arg(key).arg(value.to_string());
    }
    command
        .arg("--native-run-prompt-delivery")
        .arg(provision::DEV_RUN_PROMPT_DELIVERY);
}

/// Waits for a previous Forge still finishing its shutdown to release the
/// home's custody, which it holds until after its readiness is gone.
fn wait_for_previous(custody: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    if !custody.exists() {
        return Ok(());
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(custody)?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match fs2::FileExt::try_lock_exclusive(&file) {
            Ok(()) => return Ok(()),
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(error) => return Err(error.into()),
        }
    }
}

fn lock_supervisor(home: &std::path::Path) -> Result<std::fs::File, std::io::Error> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(home.join("supervisor.lock"))?;
    fs2::FileExt::try_lock_exclusive(&file)?;
    Ok(file)
}

#[cfg(test)]
mod restart_tests {
    use super::*;

    #[test]
    fn fatal_errors_render_their_complete_source_chain() {
        #[derive(Debug)]
        struct Wrapped(std::io::Error);
        impl std::fmt::Display for Wrapped {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("readiness failed")
            }
        }
        impl std::error::Error for Wrapped {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                Some(&self.0)
            }
        }
        let error = Wrapped(std::io::Error::other("disk full"));
        assert_eq!(error_chain(&error), "readiness failed: disk full");
    }

    #[test]
    fn one_supervisor_per_home() {
        let home = tempfile::tempdir().expect("home");
        let supervisor = lock_supervisor(home.path()).expect("first supervisor");
        assert!(lock_supervisor(home.path()).is_err());
        drop(supervisor);
        lock_supervisor(home.path()).expect("released");
    }
}
