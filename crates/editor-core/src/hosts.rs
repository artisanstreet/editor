//! The Editor's own files: its identity and the Forges it is paired with.
//! Pairing swaps certificates; the one-time code only confirms it.

use crate::client::ForgeClient;
use crate::runtime::runtime;
use anyhow::{Context, Result, bail};
use artisan_api::PairRequest;
use artisan_api::tls::Identity;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

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
    /// `host:port`.
    pub address: String,
    /// SHA-256 of Forge's certificate.
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
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct HostsFile {
    pub hosts: Vec<HostConfig>,
    /// The host to reopen.
    pub last: Option<HostId>,
}

/// `settings.json`: window and UI preferences that belong to this device.
pub type EditorSettings = serde_json::Map<String, serde_json::Value>;

/// Where the Editor keeps its files.
#[derive(Clone, Debug)]
pub struct EditorFiles {
    pub dir: PathBuf,
}

impl EditorFiles {
    #[must_use]
    pub fn default_location() -> Self {
        let dir = std::env::var_os("ARTISAN_EDITOR_DATA").map_or_else(
            || {
                dirs::config_dir()
                    .unwrap_or_else(std::env::temp_dir)
                    .join("Artisan")
                    .join("editor")
            },
            PathBuf::from,
        );
        Self { dir }
    }

    /// This device's certificate, made on first use.
    pub fn identity(&self) -> Result<Identity> {
        let path = self.dir.join("identity.json");
        match std::fs::read(&path) {
            Ok(bytes) => {
                serde_json::from_slice(&bytes).with_context(|| format!("read {}", path.display()))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let identity = Identity::generate("artisan-editor")?;
                write_json(&path, &identity)?;
                Ok(identity)
            }
            Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
        }
    }

    pub fn hosts(&self) -> Result<HostsFile> {
        read_json(&self.dir.join("hosts.json"))
    }

    pub fn save_hosts(&self, hosts: &HostsFile) -> Result<()> {
        write_json(&self.dir.join("hosts.json"), hosts)
    }

    pub fn settings(&self) -> Result<EditorSettings> {
        read_json(&self.dir.join("settings.json"))
    }

    pub fn save_settings(&self, settings: &EditorSettings) -> Result<()> {
        write_json(&self.dir.join("settings.json"), settings)
    }
}

fn read_json<T: Default + for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    match std::fs::read(path) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).with_context(|| format!("read {}", path.display()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
    }
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, serde_json::to_vec_pretty(value)?)
        .with_context(|| format!("write {}", temporary.display()))?;
    std::fs::rename(&temporary, path).with_context(|| format!("replace {}", path.display()))
}

/// What `ae forge pair --json` prints.
#[derive(Debug, Deserialize)]
struct Invite {
    code: String,
    forge_certificate: String,
    #[serde(default)]
    address: Option<String>,
}

/// Where a local Forge runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalTarget {
    ThisMachine,
    Wsl { distro: String },
}

/// Pairs with a Forge on this machine or in WSL with no typing: runs
/// `ae forge pair --json` there and uses the code it prints.
pub async fn pair_local(
    target: LocalTarget,
    identity: &Identity,
    device_name: &str,
) -> Result<HostConfig> {
    let mut command = match &target {
        LocalTarget::ThisMachine => tokio::process::Command::new("ae"),
        LocalTarget::Wsl { distro } => {
            // `wsl.exe` gives bash only the system PATH; fall back to where
            // `ae install` puts it.
            let mut command = tokio::process::Command::new("wsl.exe");
            command.args([
                "-d",
                distro,
                "--",
                "bash",
                "-c",
                r#"if command -v ae >/dev/null; then exec ae "$@"; else exec "$HOME/.local/bin/ae" "$@"; fi"#,
                "ae",
            ]);
            command
        }
    };
    command.args(["forge", "pair", "--json"]);
    let output = runtime()
        .spawn(async move { command.output().await })
        .await?
        .context("run `ae forge pair`")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // The pre-reform `ae` has no `forge` command; its usage text says
        // nothing about why pairing can't work.
        if stderr.contains("unrecognized subcommand 'forge'") {
            bail!(
                "the `ae` found there is from an older Artisan and has no Forge to pair with; install the new Artisan there first"
            );
        }
        bail!("`ae forge pair` failed: {}", stderr.trim());
    }
    let invite: Invite =
        serde_json::from_slice(&output.stdout).context("read what `ae forge pair` printed")?;
    let address = invite
        .address
        .unwrap_or_else(|| format!("127.0.0.1:{}", 47_900));
    let fingerprint = artisan_api::tls::fingerprint_of(&invite.forge_certificate)?;
    let (name, kind) = match target {
        LocalTarget::ThisMachine => ("This computer".to_owned(), HostKind::Local),
        LocalTarget::Wsl { distro } => (distro.clone(), HostKind::Wsl { distro }),
    };
    pair(
        &address,
        &invite.code,
        &fingerprint,
        identity,
        device_name,
        name,
        kind,
    )
    .await
}

/// The WSL distributions installed on this Windows machine; empty elsewhere
/// or when `wsl.exe` is missing.
pub async fn wsl_distros() -> Vec<String> {
    if !cfg!(windows) {
        return Vec::new();
    }
    let mut command = tokio::process::Command::new("wsl.exe");
    command.args(["--list", "--quiet"]);
    let output = match runtime().spawn(async move { command.output().await }).await {
        Ok(Ok(output)) if output.status.success() => output.stdout,
        Ok(Ok(output)) => {
            tracing::warn!(
                "`wsl.exe --list` failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
            return Vec::new();
        }
        Ok(Err(error)) => {
            tracing::debug!("can't run wsl.exe: {error}");
            return Vec::new();
        }
        Err(error) => {
            tracing::warn!("listing WSL distributions: {error}");
            return Vec::new();
        }
    };
    // wsl.exe prints UTF-16LE.
    let units: Vec<u16> = output
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect();
    String::from_utf16_lossy(&units)
        .lines()
        .map(|line| {
            line.trim_matches(|c: char| c.is_whitespace() || c == '\0')
                .to_owned()
        })
        .filter(|line| !line.is_empty())
        .collect()
}

/// A remote pairing string: `<host:port> <CODE> <fingerprint>` (as `ae
/// forge pair` prints it), any whitespace or `/` between the parts.
pub fn parse_pairing(text: &str) -> Result<(String, String, String)> {
    let parts: Vec<&str> = text
        .split(|c: char| c.is_whitespace() || c == '/')
        .filter(|part| !part.is_empty())
        .collect();
    match parts.as_slice() {
        [address, code, fingerprint] if fingerprint.len() == 64 => Ok((
            (*address).to_owned(),
            (*code).to_owned(),
            fingerprint.to_ascii_lowercase(),
        )),
        _ => bail!("expected `<host:port> <code> <fingerprint>` as printed by `ae forge pair`"),
    }
}

/// Pairs with the Forge at `address` using a one-time code.
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
            public_key: identity.certificate.clone(),
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
