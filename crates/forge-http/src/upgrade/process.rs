use super::{Gateway, Ready, read};
use anyhow::{Context, Result, ensure};
use forge_runs::runtime::{self, Command, Member, Registry};
use std::os::unix::fs::PermissionsExt as _;
use std::{
    fs::File,
    path::Path,
    process::{Child, Stdio},
    time::Duration,
};

pub struct ChildProcess {
    child: Option<Child>,
}

impl ChildProcess {
    pub fn release(mut self) {
        if let Some(child) = self.child.take() {
            reap(child);
        }
    }
}

impl Drop for ChildProcess {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            terminate_child(&mut child).ok();
            reap(child);
        }
    }
}

fn reap(mut child: Child) {
    if let Err(error) = std::thread::Builder::new()
        .name("forge-child".into())
        .spawn(move || {
            child.wait().ok();
        })
    {
        tracing::warn!("reap Forge child: {error}");
    }
}

pub fn listener() -> Result<std::net::TcpListener> {
    let fd = rustix::io::dup(std::io::stdin())?;
    let listener = std::net::TcpListener::from(fd);
    listener.set_nonblocking(true)?;
    Ok(listener)
}

pub fn inherited_lock() -> Result<File> {
    Ok(File::from(rustix::io::dup(std::io::stdout())?))
}

pub fn spawn_backend(binary: &Path, data: &Path, ready: &Path) -> Result<ChildProcess> {
    std::process::Command::new(binary)
        .arg("--data")
        .arg(data)
        .arg("--listen")
        .arg("127.0.0.1:0")
        .arg("--backend")
        .arg("--ready-file")
        .arg(ready)
        .env_remove("LD_LIBRARY_PATH")
        .env_remove("NOTIFY_SOCKET")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .context("start replacement backend")
        .map(|child| ChildProcess { child: Some(child) })
}

pub fn spawn_gateway(
    binary: &Path,
    data: &Path,
    handoff: &Path,
    listener: std::net::TcpListener,
    lock: File,
) -> Result<ChildProcess> {
    let listener: std::os::fd::OwnedFd = listener.into();
    std::process::Command::new(binary)
        .arg("--data")
        .arg(data)
        .arg("--handoff")
        .arg(handoff)
        .stdin(Stdio::from(listener))
        .stdout(Stdio::from(lock))
        .stderr(Stdio::inherit())
        .env_remove("LD_LIBRARY_PATH")
        .spawn()
        .context("start replacement gateway")
        .map(|child| ChildProcess { child: Some(child) })
}

pub async fn ready<T: serde::de::DeserializeOwned>(
    child: &mut ChildProcess,
    path: &Path,
) -> Result<T> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    loop {
        if let Some(status) = child
            .child
            .as_mut()
            .context("child was already released")?
            .try_wait()?
        {
            anyhow::bail!("replacement exited before becoming ready: {status}");
        }
        if let Some(value) = read(path)? {
            return Ok(value);
        }
        ensure!(
            tokio::time::Instant::now() < deadline,
            "replacement readiness timed out; current Forge remains active"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

pub async fn healthy(member: &Member) -> Result<()> {
    let value = runtime::request(member, &Command::Health).await?;
    ensure!(
        value["protocol"].as_u64() == Some(u64::from(runtime::PROTOCOL)),
        "replacement has incompatible runtime protocol"
    );
    ensure!(
        value["schema"].as_u64() == Some(forge_store::Store::schema_version() as u64),
        "replacement requires an incompatible database schema"
    );
    Ok(())
}

pub fn terminate(child: &mut ChildProcess) -> Result<()> {
    if let Some(child) = child.child.as_mut() {
        terminate_child(child)?;
    }
    Ok(())
}

fn terminate_child(child: &mut Child) -> Result<()> {
    if child.try_wait()?.is_some() {
        return Ok(());
    }
    let pid = rustix::process::Pid::from_raw(i32::try_from(child.id())?)
        .context("invalid replacement pid")?;
    rustix::process::kill_process(pid, rustix::process::Signal::TERM)?;
    Ok(())
}

pub fn notify(pid: u32) -> Result<()> {
    if std::env::var_os("NOTIFY_SOCKET").is_none() {
        return Ok(());
    }
    let status = std::process::Command::new("systemd-notify")
        .arg(format!("--pid={pid}"))
        .arg("--ready")
        .arg("--status=Forge gateway ready")
        .status()
        .context("notify the service manager of the new gateway owner")?;
    ensure!(
        status.success(),
        "service manager rejected the gateway handoff"
    );
    Ok(())
}

pub async fn candidate(
    registry: &Registry,
    data: &Path,
    binary: &Path,
    listener: std::net::TcpListener,
    lock: File,
) -> Result<Gateway> {
    ensure!(
        binary.is_absolute() && binary.is_file(),
        "replacement executable must be an absolute regular file"
    );
    let id = artisan_api::ItemId::new().to_string();
    let directory = registry.root().join("binaries").join(&id);
    std::fs::create_dir(&directory)?;
    let staged = directory.join("forge");
    std::fs::copy(binary, &staged)?;
    std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o700))?;
    let handoff = directory.join("handoff.json");
    super::write(&handoff, &serde_json::json!({"id":id}))?;
    let mut child = spawn_gateway(&staged, data, &handoff, listener, lock)?;
    let result = async {
        let Ready { gateway } = ready(&mut child, &directory.join("ready.json")).await?;
        ensure!(
            gateway.protocol == runtime::PROTOCOL,
            "replacement has incompatible gateway protocol"
        );
        healthy(&gateway.backend).await?;
        Ok(gateway)
    }
    .await;
    if result.is_err() {
        if let Ok(Some(member)) = read::<Member>(&directory.join("backend.json")) {
            runtime::request(&member, &Command::Stop).await.ok();
        }
        terminate(&mut child).ok();
    }
    if result.is_ok() {
        child.release();
    }
    result
}
