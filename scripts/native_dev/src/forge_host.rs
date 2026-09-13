//! Foreground Forge service supervisor and private invitation publisher.
use artisan_editor_cli::credentials::{self, hosts};
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
            eprintln!("Forge host: {error}");
            ExitCode::FAILURE
        }
    }
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
    let listen = resolve_address(&get("--listen")?)?;
    let advertise = resolve_address(&get("--advertise")?)?;
    if !home.is_absolute()
        || !forge.is_absolute()
        || listen.port() == 0
        || listen.port() != advertise.port()
    {
        return Err(
            "absolute paths and matching nonzero listening/advertised ports required".into(),
        );
    }
    let paths = credentials::provision_or_load(&home)?;
    let ready = home.join("readiness.json");
    // Never erase a receipt for a possibly live daemon. Service shutdown removes it.
    if ready.exists() {
        return Err("readiness already exists; stop the previous Forge and verify it has exited before removing a stale receipt".into());
    }
    let incarnation = artisan_editor_cli::instance::mint_instance_id()?;
    let mut command = Command::new(forge);
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
        while !ready.exists() {
            if child.try_wait()?.is_some() {
                return Err("Forge exited before becoming ready".into());
            }
            if start.elapsed() > Duration::from_secs(30) {
                return Err("Forge readiness timeout".into());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let invitation =
            hosts::invitation_for(&home, get("--name")?, advertise, incarnation, child.id())?;
        // Each start publishes new incarnation metadata; the stable path is replaced atomically.
        let export = home.join(format!("export-{}", child.id()));
        let source = hosts::install_private(&export, "host.json", &invitation.encode()?)?;
        std::fs::rename(source, home.join("host.json"))?;
        let _ = std::fs::remove_dir(export.join("credentials"));
        let _ = std::fs::remove_dir(export);
        eprintln!(
            "Forge ready at {advertise}; private invitation: {}",
            home.join("host.json").display()
        );
        if !child.wait()?.success() {
            return Err("Forge exited unsuccessfully".into());
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

fn resolve_address(value: &str) -> Result<SocketAddr, Box<dyn std::error::Error>> {
    let Some(port) = value.strip_prefix("auto:") else {
        return Ok(value.parse()?);
    };
    // Select the default IPv4 route without sending application data.
    let route = std::net::UdpSocket::bind((std::net::Ipv4Addr::UNSPECIFIED, 0))?;
    route.connect((std::net::Ipv4Addr::new(192, 0, 2, 1), 9))?;
    let ip = route.local_addr()?.ip();
    if ip.is_unspecified() || ip.is_loopback() || ip.is_multicast() {
        return Err("no usable default IPv4 route; configure an explicit endpoint".into());
    }
    Ok(SocketAddr::new(ip, port.parse()?))
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

fn wait_for_previous(path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    if !path.exists() {
        return Ok(());
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?;
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
