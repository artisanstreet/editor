use super::{Gateway, Ready, read};
use diagnostics::Destination;
use forge_runs::runtime::{self, Command, Member, Registry};
use std::os::unix::fs::PermissionsExt as _;
use std::{
    fs::File,
    path::{Path, PathBuf},
    process::{Child, Stdio},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

pub(super) struct ChildProcess {
    child: Option<Child>,
}

#[derive(Clone, Debug)]
pub(super) struct Exited(CancellationToken);

impl Exited {
    pub(super) async fn wait(&self) {
        self.0.cancelled().await;
    }
}

impl ChildProcess {
    pub(super) fn release(mut self) -> Exited {
        reap(
            self.child
                .take()
                .expect("invariant: a child process is only released by consuming it"),
        )
    }
}

impl Drop for ChildProcess {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            if let Err(error) = terminate_child(&mut child) {
                tracing::warn!(
                    pid = child.id(),
                    error = diagnostics::chain(&error),
                    "stop an abandoned Forge child"
                );
            }
            drop(reap(child));
        }
    }
}

fn reap(mut child: Child) -> Exited {
    let exited = CancellationToken::new();
    let reaped = exited.clone();
    let pid = child.id();
    if let Err(error) = std::thread::Builder::new()
        .name("forge-child".into())
        .spawn(move || {
            if let Err(error) = child.wait() {
                tracing::warn!(pid = child.id(), error = %error, "reap Forge child");
            }
            reaped.cancel();
        })
    {
        tracing::warn!(pid, "reap Forge child: {error}");
    }
    Exited(exited)
}

#[derive(Debug, thiserror::Error)]
pub enum InheritError {
    #[error("take the listener handed over on stdin")]
    Listener(#[source] std::io::Error),
    #[error("make the inherited listener non-blocking")]
    NonBlocking(#[source] std::io::Error),
    #[error("take the data folder lock handed over on stdout")]
    Lock(#[source] std::io::Error),
}

#[derive(Debug, thiserror::Error)]
#[error("start the replacement {role} {}", binary.display())]
pub struct SpawnError {
    role: &'static str,
    binary: PathBuf,
    #[source]
    source: std::io::Error,
}

#[derive(Debug, thiserror::Error)]
pub enum ReadyError {
    #[error("check whether the replacement process {pid} exited")]
    Wait {
        pid: u32,
        #[source]
        source: std::io::Error,
    },
    #[error("replacement process {pid} exited before becoming ready: {status}")]
    Exited {
        pid: u32,
        status: std::process::ExitStatus,
    },
    #[error("read the readiness of the replacement process {pid}")]
    Read {
        pid: u32,
        #[source]
        source: runtime::RegistryFileError,
    },
    #[error("replacement process {pid} was not ready in time; the current Forge remains active")]
    TimedOut { pid: u32 },
}

#[derive(Debug, thiserror::Error)]
pub enum HealthyError {
    #[error("ask generation {generation} for its health")]
    Request {
        generation: String,
        #[source]
        source: runtime::PeerError,
    },
    #[error("generation {generation} has an incompatible runtime protocol")]
    Protocol { generation: String },
    #[error("generation {generation} requires an incompatible database schema")]
    Schema { generation: String },
}

#[derive(Debug, thiserror::Error)]
pub(super) enum TerminateError {
    #[error("check whether process {pid} exited")]
    Wait {
        pid: u32,
        #[source]
        source: std::io::Error,
    },
    #[error("process id {pid} is not a valid signal target")]
    Pid { pid: u32 },
    #[error("send SIGTERM to process {pid}")]
    Kill {
        pid: u32,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum NotifyError {
    #[error("tell the service manager that process {pid} owns the gateway")]
    Run {
        pid: u32,
        #[source]
        source: std::io::Error,
    },
    #[error("the service manager rejected process {pid} as the gateway owner: {status}")]
    Rejected {
        pid: u32,
        status: std::process::ExitStatus,
    },
}

#[derive(Debug, thiserror::Error)]
pub(super) enum CandidateError {
    #[error("replacement executable {} must be an absolute regular file", binary.display())]
    NotAFile { binary: PathBuf },
    #[error("stage the replacement in {}", path.display())]
    Stage {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("write the handoff of replacement {id}")]
    Handoff {
        id: String,
        #[source]
        source: runtime::RegistryFileError,
    },
    #[error(transparent)]
    Spawn(#[from] SpawnError),
    #[error(transparent)]
    Ready(#[from] ReadyError),
    #[error("replacement gateway {gateway} has an incompatible gateway protocol")]
    Protocol { gateway: String },
    #[error(transparent)]
    Healthy(#[from] HealthyError),
}

pub(super) fn listener() -> Result<std::net::TcpListener, InheritError> {
    let fd =
        rustix::io::dup(std::io::stdin()).map_err(|errno| InheritError::Listener(errno.into()))?;
    let listener = std::net::TcpListener::from(fd);
    listener
        .set_nonblocking(true)
        .map_err(InheritError::NonBlocking)?;
    Ok(listener)
}

pub(super) fn inherited_lock() -> Result<File, InheritError> {
    Ok(File::from(
        rustix::io::dup(std::io::stdout()).map_err(|errno| InheritError::Lock(errno.into()))?,
    ))
}

fn logging_to(command: &mut std::process::Command, log: &Destination) {
    match log {
        Destination::Stderr => {}
        Destination::File(path) => {
            command.arg("--log").arg(path);
        }
    }
}

pub(super) fn spawn_backend(
    binary: &Path,
    data: &Path,
    log: &Destination,
    ready: &Path,
) -> Result<ChildProcess, SpawnError> {
    let mut command = std::process::Command::new(binary);
    logging_to(&mut command, log);
    command
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
        .map(|child| ChildProcess { child: Some(child) })
        .map_err(|source| SpawnError {
            role: "backend",
            binary: binary.to_owned(),
            source,
        })
}

fn spawn_gateway(
    binary: &Path,
    data: &Path,
    log: &Destination,
    handoff: &Path,
    listener: std::net::TcpListener,
    lock: File,
) -> Result<ChildProcess, SpawnError> {
    let listener: std::os::fd::OwnedFd = listener.into();
    let mut command = std::process::Command::new(binary);
    logging_to(&mut command, log);
    command
        .arg("--data")
        .arg(data)
        .arg("--handoff")
        .arg(handoff)
        .stdin(Stdio::from(listener))
        .stdout(Stdio::from(lock))
        .stderr(Stdio::inherit())
        .env_remove("LD_LIBRARY_PATH")
        .spawn()
        .map(|child| ChildProcess { child: Some(child) })
        .map_err(|source| SpawnError {
            role: "gateway",
            binary: binary.to_owned(),
            source,
        })
}

pub(super) async fn ready<T: serde::de::DeserializeOwned>(
    child: &mut ChildProcess,
    path: &Path,
) -> Result<T, ReadyError> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    let process = child
        .child
        .as_mut()
        .expect("invariant: a child process is only released by consuming it");
    let pid = process.id();
    loop {
        if let Some(status) = process
            .try_wait()
            .map_err(|source| ReadyError::Wait { pid, source })?
        {
            return Err(ReadyError::Exited { pid, status });
        }
        if let Some(value) = read(path).map_err(|source| ReadyError::Read { pid, source })? {
            return Ok(value);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(ReadyError::TimedOut { pid });
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

pub(super) async fn healthy(member: &Member) -> Result<(), HealthyError> {
    let generation = || member.id.clone();
    let value = runtime::request(member, &Command::Health)
        .await
        .map_err(|source| HealthyError::Request {
            generation: generation(),
            source,
        })?;
    if value["protocol"].as_u64() != Some(u64::from(runtime::PROTOCOL)) {
        return Err(HealthyError::Protocol {
            generation: generation(),
        });
    }
    if value["schema"].as_u64() != Some(forge_store::Store::schema_version() as u64) {
        return Err(HealthyError::Schema {
            generation: generation(),
        });
    }
    Ok(())
}

pub(super) fn terminate(child: &mut ChildProcess) -> Result<(), TerminateError> {
    if let Some(child) = child.child.as_mut() {
        terminate_child(child)?;
    }
    Ok(())
}

fn terminate_child(child: &mut Child) -> Result<(), TerminateError> {
    let pid = child.id();
    if child
        .try_wait()
        .map_err(|source| TerminateError::Wait { pid, source })?
        .is_some()
    {
        return Ok(());
    }
    let Some(target) = i32::try_from(pid).map_or(None, rustix::process::Pid::from_raw) else {
        return Err(TerminateError::Pid { pid });
    };
    rustix::process::kill_process(target, rustix::process::Signal::TERM).map_err(|errno| {
        TerminateError::Kill {
            pid,
            source: errno.into(),
        }
    })?;
    Ok(())
}

pub(super) fn notify(pid: u32) -> Result<(), NotifyError> {
    if std::env::var_os("NOTIFY_SOCKET").is_none() {
        return Ok(());
    }
    let status = std::process::Command::new("systemd-notify")
        .arg(format!("--pid={pid}"))
        .arg("--ready")
        .arg("--status=Forge gateway ready")
        .status()
        .map_err(|source| NotifyError::Run { pid, source })?;
    if !status.success() {
        return Err(NotifyError::Rejected { pid, status });
    }
    Ok(())
}

pub(super) async fn candidate(
    registry: &Registry,
    data: &Path,
    log: &Destination,
    binary: &Path,
    listener: std::net::TcpListener,
    lock: File,
) -> Result<Gateway, CandidateError> {
    if !(binary.is_absolute() && binary.is_file()) {
        return Err(CandidateError::NotAFile {
            binary: binary.to_owned(),
        });
    }
    let id = artisan_api::ItemId::new().to_string();
    let directory = registry.root().join("binaries").join(&id);
    let staged = directory.join("forge");
    let stage = |path: &Path| {
        let path = path.to_owned();
        move |source| CandidateError::Stage { path, source }
    };
    std::fs::create_dir(&directory).map_err(stage(&directory))?;
    std::fs::copy(binary, &staged).map_err(stage(&staged))?;
    std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o700))
        .map_err(stage(&staged))?;
    let handoff = directory.join("handoff.json");
    super::write(&handoff, &serde_json::json!({"id":id})).map_err(|source| {
        CandidateError::Handoff {
            id: id.clone(),
            source,
        }
    })?;
    let mut child = spawn_gateway(&staged, data, log, &handoff, listener, lock)?;
    let result = async {
        let Ready { gateway } = ready(&mut child, &directory.join("ready.json")).await?;
        if gateway.protocol != runtime::PROTOCOL {
            return Err(CandidateError::Protocol {
                gateway: gateway.id,
            });
        }
        healthy(&gateway.backend).await?;
        Ok(gateway)
    }
    .await;
    match &result {
        Ok(_) => drop(child.release()),
        Err(_) => abandon(&mut child, &directory.join("backend.json")).await,
    }
    result
}

async fn abandon(child: &mut ChildProcess, backend: &Path) {
    match read::<Member>(backend) {
        Ok(Some(member)) => {
            if let Err(error) = runtime::request(&member, &Command::Stop).await {
                tracing::warn!(generation = %member.id, error = diagnostics::chain(&error), "stop the abandoned replacement backend");
            }
        }
        Ok(None) => {}
        Err(error) => {
            tracing::warn!(
                error = diagnostics::chain(&error),
                "read the abandoned replacement backend"
            );
        }
    }
    if let Err(error) = terminate(child) {
        tracing::warn!(
            error = diagnostics::chain(&error),
            "stop the abandoned replacement gateway"
        );
    }
}
