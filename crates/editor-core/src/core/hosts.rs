use crate::core::ae::{Ae, AeError};
use crate::core::client::{CallError, ForgeClient, NewClientError};
use crate::core::config_file::{self, ReadConfigError, UpdateConfigError, WriteConfigError};
use crate::core::runtime::runtime;
use artisan_api::PairRequest;
use artisan_api::tls::Identity;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HostId(pub String);

impl std::fmt::Display for HostId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostConfig {
    pub id: HostId,
    pub name: String,
    pub address: String,
    pub fingerprint: String,
    #[serde(default)]
    pub kind: HostKind,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostKind {
    #[default]
    Local,
    Wsl {
        distro: String,
    },
    Remote,
    Account {
        account: String,
        machine: String,
    },
}

#[must_use]
pub fn is_loopback(address: &str) -> bool {
    match url::Url::parse(&format!("https://{address}")) {
        Ok(url) => match url.host() {
            Some(url::Host::Domain(domain)) => domain == "localhost",
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            None => false,
        },
        Err(_) => false,
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct HostsFile {
    pub hosts: Vec<HostConfig>,
    pub last: Option<HostId>,
}

pub type EditorSettings = serde_json::Map<String, serde_json::Value>;

#[derive(Clone, Debug)]
pub struct EditorFiles {
    pub dir: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum DataDirError {
    #[error("find the Editor's data directory")]
    Resolve(#[source] artisan_paths::RootError),
}

#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    #[error("read this device's identity")]
    Read(#[source] ReadConfigError),
    #[error("generate this device's identity")]
    Generate(#[source] artisan_api::tls::GenerateIdentityError),
    #[error("save this device's new identity")]
    Save(#[source] WriteConfigError),
    #[error("{path} was removed while another Editor created it")]
    Vanished { path: PathBuf },
}

impl EditorFiles {
    pub fn default_location() -> Result<Self, DataDirError> {
        let dir = match std::env::var_os("ARTISAN_EDITOR_DATA") {
            Some(dir) => PathBuf::from(dir),
            None => artisan_paths::App::Editor
                .data()
                .map_err(DataDirError::Resolve)?,
        };
        Ok(Self { dir })
    }

    pub fn identity(&self) -> Result<Identity, IdentityError> {
        let path = self.dir.join("identity.json");
        if let Some(identity) =
            config_file::read::<Option<Identity>>(&path).map_err(IdentityError::Read)?
        {
            return Ok(identity);
        }
        let identity = Identity::generate("artisan-editor").map_err(IdentityError::Generate)?;
        match config_file::create(&path, &identity) {
            Ok(()) => Ok(identity),
            Err(WriteConfigError::AlreadyExists { .. }) => {
                config_file::read::<Option<Identity>>(&path)
                    .map_err(IdentityError::Read)?
                    .ok_or(IdentityError::Vanished { path })
            }
            Err(error) => Err(IdentityError::Save(error)),
        }
    }

    pub fn hosts(&self) -> Result<HostsFile, ReadConfigError> {
        config_file::read(&self.hosts_path())
    }

    pub fn save_hosts(&self, hosts: &HostsFile) -> Result<(), WriteConfigError> {
        config_file::write(&self.hosts_path(), hosts)
    }

    pub fn update_hosts(
        &self,
        change: impl FnOnce(&mut HostsFile),
    ) -> Result<HostsFile, UpdateConfigError> {
        config_file::update(&self.hosts_path(), change).map(|(hosts, ())| hosts)
    }

    pub(crate) fn settings(&self) -> Result<EditorSettings, ReadConfigError> {
        config_file::read(&self.settings_path())
    }

    pub(crate) fn update_settings(
        &self,
        change: impl FnOnce(&mut EditorSettings),
    ) -> Result<EditorSettings, UpdateConfigError> {
        config_file::update(&self.settings_path(), change).map(|(settings, ())| settings)
    }

    fn hosts_path(&self) -> PathBuf {
        self.dir.join("hosts.json")
    }

    fn settings_path(&self) -> PathBuf {
        self.dir.join("settings.json")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PairThisMachineError {
    #[error("ask `ae pair` for an invite to {host}")]
    Invite {
        host: HostId,
        #[source]
        source: AeError,
    },
    #[error("read the invite `ae pair` printed for {host}")]
    ReadInvite {
        host: HostId,
        #[source]
        source: serde_json::Error,
    },
    #[error("read the certificate in the invite for {host}")]
    Certificate {
        host: HostId,
        #[source]
        source: artisan_api::tls::DecodeCertificateError,
    },
    #[error("pair with this machine's Forge as {host}")]
    Pair {
        host: HostId,
        #[source]
        source: PairError,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum PairError {
    #[error("connect to Forge at {address}")]
    Client {
        address: String,
        #[source]
        source: Box<NewClientError>,
    },
    #[error("pair with Forge at {address}")]
    Pair {
        address: String,
        #[source]
        source: CallError,
    },
    #[error("Forge at {address} has fingerprint {fingerprint:?}, which is too short to name it")]
    Fingerprint {
        address: String,
        fingerprint: String,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum WslCommandError {
    #[error("the wsl.exe task stopped")]
    Task(#[source] tokio::task::JoinError),
    #[error("wsl.exe did not finish within {seconds} seconds")]
    TimedOut { seconds: u64 },
    #[error("run wsl.exe")]
    Run(#[source] std::io::Error),
    #[error("wsl.exe exited with {code:?}: {stderr}")]
    Failed { code: Option<i32>, stderr: String },
}

#[derive(Debug, thiserror::Error)]
#[error("read WSL's state")]
pub struct WslStateError(#[source] WslCommandError);

#[derive(Debug, thiserror::Error)]
#[error("start Forge with `ae forge start` in {distro}")]
pub struct StartWslForgeError {
    distro: String,
    #[source]
    source: WslCommandError,
}

#[derive(Debug, thiserror::Error)]
#[error("keep {distro} awake")]
pub struct KeepWslAwakeError {
    distro: String,
    #[source]
    source: std::io::Error,
}

#[derive(Debug, Deserialize)]
struct Invite {
    code: String,
    forge_certificate: String,
    address: String,
}

pub(crate) async fn pair_this_machine(
    ae: &Ae,
    identity: &Identity,
    device_name: &str,
    id: HostId,
    name: String,
) -> Result<HostConfig, PairThisMachineError> {
    let printed =
        ae.run(&["pair", "--json"])
            .await
            .map_err(|source| PairThisMachineError::Invite {
                host: id.clone(),
                source,
            })?;
    let invite: Invite =
        serde_json::from_str(&printed).map_err(|source| PairThisMachineError::ReadInvite {
            host: id.clone(),
            source,
        })?;
    let fingerprint =
        artisan_api::tls::fingerprint_of(&invite.forge_certificate).map_err(|source| {
            PairThisMachineError::Certificate {
                host: id.clone(),
                source,
            }
        })?;
    let config = pair(
        &invite.address,
        &invite.code,
        &fingerprint,
        identity,
        device_name,
        name,
        HostKind::Local,
    )
    .await
    .map_err(|source| PairThisMachineError::Pair {
        host: id.clone(),
        source,
    })?;
    Ok(HostConfig { id, ..config })
}

const AE_IN_WSL: &str = r#"exec "$HOME/.local/bin/ae" "$@""#;

fn wsl_exe() -> tokio::process::Command {
    let mut command = tokio::process::Command::new("wsl.exe");
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command.kill_on_drop(true);
    command
}

async fn run_wsl(
    mut command: tokio::process::Command,
    seconds: u64,
) -> Result<std::process::Output, WslCommandError> {
    let output = runtime()
        .spawn(async move {
            tokio::time::timeout(std::time::Duration::from_secs(seconds), command.output()).await
        })
        .await
        .map_err(WslCommandError::Task)?
        .map_err(|_elapsed| WslCommandError::TimedOut { seconds })?
        .map_err(WslCommandError::Run)?;
    if !output.status.success() {
        return Err(WslCommandError::Failed {
            code: output.status.code(),
            stderr: wsl_text(&output.stderr).trim().to_owned(),
        });
    }
    Ok(output)
}

async fn wsl_listing(args: &'static [&'static str]) -> Result<String, WslCommandError> {
    let mut command = wsl_exe();
    command.args(args);
    let output = run_wsl(command, 10).await?;
    Ok(wsl_text(&output.stdout))
}

fn wsl_text(bytes: &[u8]) -> String {
    let text = if bytes.contains(&0) {
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    };
    text.trim_start_matches('\u{feff}').replace('\0', "")
}

fn running_or_none(listing: Result<String, WslCommandError>) -> Result<String, WslCommandError> {
    match listing {
        Err(WslCommandError::Failed { code, stderr }) => {
            tracing::info!(
                ?code,
                %stderr,
                "wsl.exe --list --running failed, so no distribution is running"
            );
            Ok(String::new())
        }
        listing => listing,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WslDistro {
    pub name: String,
    pub default: bool,
    pub running: bool,
}

pub fn parse_wsl_listing(verbose: &str, running: &str) -> Vec<WslDistro> {
    let running: std::collections::HashSet<&str> = running.lines().map(str::trim).collect();
    verbose
        .lines()
        .skip(1)
        .filter_map(|line| {
            let line = line.trim();
            let (default, line) = line
                .strip_prefix('*')
                .map_or((false, line), |rest| (true, rest));

            let mut fields = line.split_whitespace();
            let name = fields.next()?;
            fields.next()?;
            Some(WslDistro {
                name: name.to_owned(),
                default,
                running: running.contains(name),
            })
        })
        .collect()
}

pub async fn wsl_state() -> Result<Vec<WslDistro>, WslStateError> {
    if !cfg!(windows) {
        return Ok(Vec::new());
    }
    let (verbose, running) = tokio::join!(
        wsl_listing(&["--list", "--verbose"]),
        wsl_listing(&["--list", "--running", "--quiet"]),
    );
    let verbose = verbose.map_err(WslStateError)?;
    let running = running_or_none(running).map_err(WslStateError)?;
    Ok(parse_wsl_listing(&verbose, &running))
}

pub(crate) async fn start_wsl_forge(distro: String) -> Result<(), StartWslForgeError> {
    let mut command = wsl_exe();
    command.args([
        "-d", &distro, "--exec", "sh", "-c", AE_IN_WSL, "ae", "forge", "start",
    ]);
    run_wsl(command, 60)
        .await
        .map(drop)
        .map_err(|source| StartWslForgeError { distro, source })
}

pub(crate) fn keep_wsl_awake(distro: &str) -> Result<tokio::process::Child, KeepWslAwakeError> {
    let _runtime = runtime().enter();
    wsl_exe()
        .args(["-d", distro, "--exec", "sleep", "infinity"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|source| KeepWslAwakeError {
            distro: distro.to_owned(),
            source,
        })
}

pub async fn pair(
    address: &str,
    code: &str,
    fingerprint: &str,
    identity: &Identity,
    device_name: &str,
    name: String,
    kind: HostKind,
) -> Result<HostConfig, PairError> {
    let short = fingerprint.get(..8).ok_or_else(|| PairError::Fingerprint {
        address: address.to_owned(),
        fingerprint: fingerprint.to_owned(),
    })?;
    let client =
        ForgeClient::new(address, fingerprint, identity).map_err(|source| PairError::Client {
            address: address.to_owned(),
            source: Box::new(source),
        })?;
    client
        .pair(&PairRequest {
            code: code.to_owned(),
            device_name: device_name.to_owned(),
        })
        .await
        .map_err(|source| PairError::Pair {
            address: address.to_owned(),
            source,
        })?;
    Ok(HostConfig {
        id: HostId(format!("{}-{short}", address.replace([':', '.'], "-"))),
        name,
        address: address.to_owned(),
        fingerprint: fingerprint.to_owned(),
        kind,
    })
}

#[cfg(test)]
#[path = "../../tests/core/hosts/wsl_command.rs"]
mod wsl_command_tests;
