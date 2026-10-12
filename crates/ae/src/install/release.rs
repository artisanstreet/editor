//! Releases: a signed `release.json` naming one archive per platform.
//!
//! ```json
//! {"version": "0.5.0", "artifacts": [
//!   {"os": "linux", "arch": "x86_64", "file": "artisan-0.5.0-linux-x86_64.tar.gz", "sha256": "…"}]}
//! ```
//!
//! `release.json.sig` holds the base64 Ed25519 signature of the exact
//! `release.json` bytes; each archive must match its SHA-256 there. Archives
//! (`.tar.gz`, or `.zip` for Windows) hold `ae` and, where there is a
//! desktop, `artisan-editor`, at the top level.

use anyhow::{Context, Result, bail, ensure};
use base64::Engine as _;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

pub const DEFAULT_FEED: &str =
    "https://github.com/sandersonstabo/artisan-editor/releases/latest/download/release.json";
const MANIFEST: &str = "release.json";

/// The release signing key (raw Ed25519, hex). Release builds set
/// `ARTISAN_RELEASE_PUBLIC_KEY_HEX`; the fallback is the development key.
const PUBLIC_KEY_HEX: &str = match option_env!("ARTISAN_RELEASE_PUBLIC_KEY_HEX") {
    Some(key) => key,
    None => "ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c",
};

#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub version: String,
    pub artifacts: Vec<Artifact>,
}

#[derive(Debug, Deserialize)]
pub struct Artifact {
    pub os: String,
    pub arch: String,
    pub file: String,
    pub sha256: String,
}

/// Where releases come from: a `release.json` URL or a directory holding
/// one (with its signature and archives).
#[derive(Clone, Debug)]
pub enum Source {
    Url(String),
    Dir(PathBuf),
}

impl Source {
    /// `--from`, else `ARTISAN_RELEASES`, else the public feed.
    pub fn new(from: Option<&str>) -> Self {
        let from = from.map(str::to_owned).or_else(|| {
            std::env::var("ARTISAN_RELEASES")
                .ok()
                .filter(|value| !value.is_empty())
        });
        match from {
            Some(value) if value.starts_with("https://") || value.starts_with("http://") => {
                Self::Url(value)
            }
            Some(value) => Self::Dir(PathBuf::from(value)),
            None => Self::Url(DEFAULT_FEED.to_owned()),
        }
    }

    async fn fetch(&self, http: &reqwest::Client, name: &str) -> Result<Vec<u8>> {
        match self {
            Self::Dir(dir) => {
                let path = dir.join(name);
                std::fs::read(&path).with_context(|| format!("read {}", path.display()))
            }
            Self::Url(manifest) => {
                let url = match name {
                    MANIFEST => manifest.clone(),
                    _ if name == format!("{MANIFEST}.sig") => format!("{manifest}.sig"),
                    _ => format!(
                        "{}/{name}",
                        manifest
                            .rsplit_once('/')
                            .map_or(manifest.as_str(), |(base, _)| base)
                    ),
                };
                let response = http
                    .get(&url)
                    .send()
                    .await
                    .with_context(|| format!("download {url}"))?;
                let response = response
                    .error_for_status()
                    .with_context(|| format!("download {url}"))?;
                Ok(response
                    .bytes()
                    .await
                    .with_context(|| format!("download {url}"))?
                    .to_vec())
            }
        }
    }
}

pub struct Release {
    pub manifest: Manifest,
    raw: Vec<u8>,
    signature: Vec<u8>,
    source: Source,
    http: reqwest::Client,
}

/// Fetches and verifies the release manifest.
pub async fn resolve(source: Source) -> Result<Release> {
    let http = reqwest::Client::builder()
        .user_agent(concat!("ae/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("start the HTTP client")?;
    let raw = source.fetch(&http, MANIFEST).await?;
    let signature = source.fetch(&http, &format!("{MANIFEST}.sig")).await?;
    verify_signature(&raw, &signature)
        .with_context(|| format!("verify the release from {source:?}"))?;
    let manifest: Manifest = serde_json::from_slice(&raw).context("read release.json")?;
    ensure!(
        usable_name(&manifest.version),
        "unusable release version {:?}",
        manifest.version
    );
    Ok(Release {
        manifest,
        raw,
        signature,
        source,
        http,
    })
}

fn verify_signature(raw: &[u8], signature: &[u8]) -> Result<()> {
    let key_hex = if cfg!(debug_assertions) {
        // Lets tests and development builds use their own key.
        std::env::var("ARTISAN_RELEASE_PUBLIC_KEY_HEX")
            .unwrap_or_else(|_| PUBLIC_KEY_HEX.to_owned())
    } else {
        PUBLIC_KEY_HEX.to_owned()
    };
    let key: [u8; 32] = hex(&key_hex)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("the release key is not 32 bytes"))?;
    let key = ed25519_dalek::VerifyingKey::from_bytes(&key).context("read the release key")?;
    let text = String::from_utf8_lossy(signature);
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(text.trim())
        .context("decode release.json.sig")?;
    let signature =
        ed25519_dalek::Signature::from_slice(&bytes).context("read release.json.sig")?;
    key.verify_strict(raw, &signature)
        .context("release.json is not signed by Artisan's release key")
}

impl Release {
    pub fn version(&self) -> &str {
        &self.manifest.version
    }

    pub fn artifact(&self, os: &str, arch: &str) -> Result<&Artifact> {
        self.manifest
            .artifacts
            .iter()
            .find(|artifact| artifact.os == os && artifact.arch == arch)
            .with_context(|| {
                format!(
                    "release {} has no build for {os} {arch}",
                    self.manifest.version
                )
            })
    }

    /// Downloads the archive and checks it against the signed manifest.
    pub async fn download(&self, artifact: &Artifact) -> Result<Vec<u8>> {
        ensure!(
            usable_name(&artifact.file),
            "unusable archive name {:?}",
            artifact.file
        );
        eprintln!("Downloading {}…", artifact.file);
        let bytes = self.source.fetch(&self.http, &artifact.file).await?;
        let actual = Sha256::digest(&bytes)
            .iter()
            .fold(String::new(), |mut hex, byte| {
                let _ = write!(hex, "{byte:02x}");
                hex
            });
        if !actual.eq_ignore_ascii_case(&artifact.sha256) {
            bail!(
                "{} is corrupt or was tampered with (SHA-256 {actual}, expected {})",
                artifact.file,
                artifact.sha256
            );
        }
        Ok(bytes)
    }

    /// Copies the manifest, its signature and one platform's archive into
    /// `dir`, so another host (WSL, Windows) can install from it offline.
    pub async fn stage(&self, artifact: &Artifact, dir: &Path) -> Result<Vec<u8>> {
        let archive = self.download(artifact).await?;
        std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        std::fs::write(dir.join(MANIFEST), &self.raw)?;
        std::fs::write(dir.join(format!("{MANIFEST}.sig")), &self.signature)?;
        std::fs::write(dir.join(&artifact.file), &archive)
            .with_context(|| format!("stage {}", artifact.file))?;
        Ok(archive)
    }
}

/// Unpacks a verified archive into `dest` (which must not exist).
pub fn extract(file_name: &str, bytes: &[u8], dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest).with_context(|| format!("create {}", dest.display()))?;
    if Path::new(file_name)
        .extension()
        .is_some_and(|extension| extension == "zip")
    {
        zip::ZipArchive::new(std::io::Cursor::new(bytes))
            .and_then(|mut archive| archive.extract(dest))
            .with_context(|| format!("unpack {file_name}"))
    } else {
        tar::Archive::new(flate2::read::GzDecoder::new(bytes))
            .unpack(dest)
            .with_context(|| format!("unpack {file_name}"))
    }
}

/// Versions and file names become path components.
pub fn usable_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b".-_+".contains(&byte))
}

fn hex(text: &str) -> Result<Vec<u8>> {
    let text = text.trim();
    ensure!(text.len().is_multiple_of(2), "odd-length hex");
    text.as_bytes()
        .chunks(2)
        .map(|pair| {
            std::str::from_utf8(pair)
                .ok()
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
                .context("invalid hex")
        })
        .collect()
}

/// `std::env::consts` names, as release.json uses them.
pub fn this_platform() -> (&'static str, &'static str) {
    (std::env::consts::OS, std::env::consts::ARCH)
}

/// Orders dotted versions numerically (`0.10.0` > `0.9.1`).
pub fn newer(candidate: &str, current: &str) -> bool {
    let parts = |version: &str| -> Vec<u64> {
        version
            .split(|c: char| !c.is_ascii_digit())
            .filter_map(|part| part.parse().ok())
            .collect()
    };
    parts(candidate) > parts(current)
}
