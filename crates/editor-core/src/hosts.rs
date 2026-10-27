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
    /// Paired through the AST account Relay, not direct mTLS. Rebuilt
    /// from the account session on resume (never dialed as `host:port`):
    /// `address` and `fingerprint` stay empty for these entries. The
    /// Forge's enrolled Ed25519 key is Noise-pinning identity material
    /// presented in-band, never a TLS fingerprint — never store it as one.
    Account { account: String, machine: String },
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
        // `ARTISAN_EDITOR_DATA` is for capture and test runs only.
        let dir = std::env::var_os("ARTISAN_EDITOR_DATA").map_or_else(
            || {
                artisan_paths::App::Editor
                    .data()
                    .unwrap_or_else(|_| std::env::temp_dir().join("artisan-editor"))
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
    use std::io::Write as _;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let parent = path.parent().context("configuration directory")?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(parent).context("create configuration temporary")?;
    serde_json::to_writer_pretty(temporary.as_file_mut(), value)?;
    temporary.flush()?;
    temporary
        .persist(path)
        .with_context(|| format!("replace {}", path.display()))?;
    Ok(())
}

/// What `ae pair --json` prints.
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
/// `ae pair --json` there and uses the code it prints.
pub async fn pair_local(
    target: LocalTarget,
    identity: &Identity,
    device_name: &str,
) -> Result<HostConfig> {
    let mut command = match &target {
        LocalTarget::ThisMachine => tokio::process::Command::new("ae"),
        LocalTarget::Wsl { distro } => {
            let mut command = wsl_exe();
            command.args(["-d", distro, "--exec", "sh", "-c", AE_IN_WSL, "ae"]);
            command
        }
    };
    command.args(["pair", "--json"]);
    let output = runtime()
        .spawn(async move { command.output().await })
        .await?
        .context("run `ae pair`")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // An older `ae` has no `pair` command; its usage text says nothing
        // about why pairing can't work.
        if stderr.contains("unrecognized subcommand 'pair'") {
            bail!(
                "the `ae` found there is from an older Artisan; install the new Artisan there first (`ae install`)"
            );
        }
        bail!("`ae pair` failed: {}", stderr.trim());
    }
    let invite: Invite =
        serde_json::from_slice(&output.stdout).context("read what `ae pair` printed")?;
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

/// Runs `ae` inside a WSL distribution, where `ae install` links it.
const AE_IN_WSL: &str = r#"exec "$HOME/.local/bin/ae" "$@""#;

/// `wsl.exe` without a console window: the Editor is a GUI app, so a console
/// child would otherwise flash one.
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

/// Runs `wsl.exe` with `args` and returns what it printed. `wsl.exe` prints
/// its own listings in UTF-16LE.
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

/// The WSL distributions installed on this Windows machine; empty elsewhere
/// or when `wsl.exe` is missing.
pub(crate) async fn wsl_distros() -> Result<Vec<String>> {
    if !cfg!(windows) {
        return Ok(Vec::new());
    }
    Ok(wsl_listing(&["--list", "--quiet"])
        .await
        .context("Could not list WSL distributions")?
        .lines()
        .map(|line| line.trim().to_owned())
        .filter(|line| !line.is_empty())
        .collect())
}

/// One row of `wsl.exe --list --verbose`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WslDistro {
    pub name: String,
    pub default: bool,
    pub running: bool,
}

/// Reads `wsl.exe --list --verbose`: `* Ubuntu  Running  2`, the default
/// marked with `*`. Distribution names hold no spaces.
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

/// The installed distributions and which run; empty off Windows.
pub(crate) async fn wsl_state() -> Result<Vec<WslDistro>> {
    if !cfg!(windows) {
        return Ok(Vec::new());
    }
    Ok(parse_wsl_verbose(
        &wsl_listing(&["--list", "--verbose"])
            .await
            .context("Could not read WSL's state")?,
    ))
}

/// Starts `distro` and the Forge installed in it (`ae forge start`, which
/// does nothing when Forge already runs).
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

/// Holds a session open in `distro`, so WSL doesn't shut it down (and the
/// Forge in it) once no terminal is open. Dropping the child ends it.
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

/// A remote pairing string: `<host:port> <CODE> <fingerprint>` (as `ae
/// pair` prints it), any whitespace or `/` between the parts.
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
        _ => bail!("expected `<host:port> <code> <fingerprint>` as printed by `ae pair`"),
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
