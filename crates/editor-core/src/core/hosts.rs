use crate::core::ae::Ae;
use crate::core::client::ForgeClient;
use crate::core::config_file::{self, ReadConfigError, UpdateConfigError, WriteConfigError};
use crate::core::runtime::runtime;
use anyhow::{Context, Result, bail};
use artisan_api::PairRequest;
use artisan_api::tls::Identity;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HostId(pub String);

impl std::fmt::Display for HostId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
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
    Generate(#[source] anyhow::Error),
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

    pub fn settings(&self) -> Result<EditorSettings, ReadConfigError> {
        config_file::read(&self.settings_path())
    }

    pub fn update_settings(
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
) -> Result<HostConfig> {
    let invite: Invite = serde_json::from_str(&ae.run(&["pair", "--json"]).await?)
        .context("read what `ae pair` printed")?;
    let fingerprint = artisan_api::tls::fingerprint_of(&invite.forge_certificate)?;
    let config = pair(
        &invite.address,
        &invite.code,
        &fingerprint,
        identity,
        device_name,
        name,
        HostKind::Local,
    )
    .await?;
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

async fn wsl_listing(args: &'static [&'static str]) -> Result<String> {
    let mut command = wsl_exe();
    command.args(args);
    let output = runtime()
        .spawn(async move {
            tokio::time::timeout(std::time::Duration::from_secs(10), command.output()).await
        })
        .await
        .context("WSL query task failed")?
        .context("WSL query timed out")?
        .context("Could not run wsl.exe")?;
    anyhow::ensure!(
        output.status.success(),
        "wsl.exe failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let units: Vec<u16> = output
        .stdout
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect();
    Ok(String::from_utf16_lossy(&units).replace('\0', ""))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WslDistro {
    pub name: String,
    pub default: bool,
    pub running: bool,
}

pub(crate) fn parse_wsl_verbose(text: &str) -> Vec<WslDistro> {
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let line = line.trim();
            let (default, line) = line
                .strip_prefix('*')
                .map_or((false, line), |rest| (true, rest));

            let mut fields = line.split_whitespace();
            let name = fields.next()?.to_owned();
            let state = fields.next()?;
            Some(WslDistro {
                name,
                default,
                running: state.eq_ignore_ascii_case("running"),
            })
        })
        .collect()
}

pub async fn wsl_state() -> Result<Vec<WslDistro>> {
    if !cfg!(windows) {
        return Ok(Vec::new());
    }
    Ok(parse_wsl_verbose(
        &wsl_listing(&["--list", "--verbose"])
            .await
            .context("Could not read WSL's state")?,
    ))
}

pub(crate) async fn start_wsl_forge(distro: String) -> Result<()> {
    let mut command = wsl_exe();
    command.args([
        "-d", &distro, "--exec", "sh", "-c", AE_IN_WSL, "ae", "forge", "start",
    ]);
    let output = runtime()
        .spawn(async move {
            tokio::time::timeout(std::time::Duration::from_secs(60), command.output()).await
        })
        .await
        .context("WSL start task failed")?
        .context("starting WSL timed out")?
        .context("Could not run wsl.exe")?;
    if !output.status.success() {
        bail!(
            "`ae forge start` failed in {distro}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

pub(crate) fn keep_wsl_awake(distro: &str) -> Result<tokio::process::Child> {
    let _runtime = runtime().enter();
    wsl_exe()
        .args(["-d", distro, "--exec", "sleep", "infinity"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("keep WSL awake")
}

pub async fn pair(
    address: &str,
    code: &str,
    fingerprint: &str,
    identity: &Identity,
    device_name: &str,
    name: String,
    kind: HostKind,
) -> Result<HostConfig> {
    let client = ForgeClient::new(address, fingerprint, identity)?;
    client
        .pair(&PairRequest {
            code: code.to_owned(),
            device_name: device_name.to_owned(),
        })
        .await
        .with_context(|| format!("pair with Forge at {address}"))?;
    Ok(HostConfig {
        id: HostId(format!(
            "{}-{}",
            address.replace([':', '.'], "-"),
            &fingerprint[..8]
        )),
        name,
        address: address.to_owned(),
        fingerprint: fingerprint.to_owned(),
        kind,
    })
}
