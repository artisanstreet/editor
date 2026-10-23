//! Where apps come from: a signed release feed, or a local build.
//!
//! A feed is a `release.json` with one archive per app and platform:
//!
//! ```json
//! {"version": "0.5.0", "artifacts": [
//!   {"app": "forge", "os": "linux", "arch": "x86_64", "file": "forge-0.5.0-linux-x86_64.tar.gz", "sha256": "…"}]}
//! ```
//!
//! `release.json.sig` holds the base64 Ed25519 signature of the exact
//! `release.json` bytes; each archive must match its SHA-256 there. An
//! archive (`.tar.gz`, `.zip` for Windows) holds the app's binary and its
//! notices at the top level.
//!
//! A local build (`--from <dir>` without a `release.json`) is a folder per
//! OS (`linux/`, `windows/`, `macos/`) holding the binaries and a `VERSION`
//! file; it is what `nix run .#dev` installs.

use crate::ui::Progress;
use anyhow::{Context, Result, bail, ensure};
use artisan_paths::App;
use base64::Engine as _;
use serde::Deserialize;
use sha2::{Digest, Sha256};
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
    version: String,
    artifacts: Vec<Artifact>,
}

#[derive(Debug, Deserialize)]
pub struct Artifact {
    app: String,
    os: String,
    arch: String,
    file: String,
    sha256: String,
}

/// `--from`, else the public feed (`--version` picks that release on it).
#[derive(Clone, Debug)]
pub enum Source {
    Url(String),
    Dir(PathBuf),
}

impl Source {
    pub fn new(from: Option<&str>, version: Option<&str>) -> Result<Self> {
        Ok(match (from, version) {
            (Some(_), Some(_)) => bail!("pass either --from or --version"),
            (Some(value), None)
                if value.starts_with("https://") || value.starts_with("http://") =>
            {
                Self::Url(value.to_owned())
            }
            (Some(value), None) => {
                Self::Dir(std::path::absolute(value).with_context(|| format!("resolve {value}"))?)
            }
            (None, Some(version)) => {
                ensure!(usable_name(version), "unusable version {version:?}");
                Self::Url(
                    DEFAULT_FEED.replace("/latest/download/", &format!("/download/v{version}/")),
                )
            }
            (None, None) => Self::Url(DEFAULT_FEED.to_owned()),
        })
    }

    /// The `--from` value that names this source to another `ae`.
    pub fn argument(&self) -> String {
        match self {
            Self::Url(url) => url.clone(),
            Self::Dir(dir) => dir.display().to_string(),
        }
    }

    fn file(&self, name: &str) -> String {
        match self {
            Self::Dir(dir) => dir.join(name).display().to_string(),
            Self::Url(manifest) => match name {
                MANIFEST => manifest.clone(),
                _ => format!(
                    "{}/{name}",
                    manifest
                        .rsplit_once('/')
                        .map_or(manifest.as_str(), |(base, _)| base)
                ),
            },
        }
    }
}

/// A resolved source: a verified feed, or a local build.
pub enum Release {
    Feed {
        manifest: Manifest,
        source: Source,
        http: reqwest::Client,
    },
    Local {
        dir: PathBuf,
    },
}

/// Fetches and verifies the feed, or recognises a local build.
pub async fn resolve(source: &Source) -> Result<Release> {
    if let Source::Dir(dir) = source
        && !dir.join(MANIFEST).is_file()
    {
        ensure!(
            dir.is_dir(),
            "{} is neither a release nor a local build",
            dir.display()
        );
        return Ok(Release::Local { dir: dir.clone() });
    }
    let http = reqwest::Client::builder()
        .user_agent(concat!("ae/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("start the HTTP client")?;
    let raw = fetch(&http, source, MANIFEST, None).await?;
    let signature = fetch(&http, source, &format!("{MANIFEST}.sig"), None).await?;
    verify_signature(&raw, &signature)
        .with_context(|| format!("verify the release from {}", source.argument()))?;
    let manifest: Manifest = serde_json::from_slice(&raw).context("read release.json")?;
    ensure!(
        usable_name(&manifest.version),
        "unusable release version {:?}",
        manifest.version
    );
    Ok(Release::Feed {
        manifest,
        source: source.clone(),
        http,
    })
}

async fn fetch(
    http: &reqwest::Client,
    source: &Source,
    name: &str,
    progress: Option<&Progress>,
) -> Result<Vec<u8>> {
    let location = source.file(name);
    match source {
        Source::Dir(_) => std::fs::read(&location).with_context(|| format!("read {location}")),
        Source::Url(_) => {
            let mut response = http
                .get(&location)
                .send()
                .await
                .and_then(reqwest::Response::error_for_status)
                .with_context(|| format!("download {location}"))?;
            let mut bytes = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .with_context(|| format!("download {location}"))?
            {
                if let Some(progress) = progress {
                    progress.advance(chunk.len() as u64);
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(bytes)
        }
    }
}

fn verify_signature(raw: &[u8], signature: &[u8]) -> Result<()> {
    let key_hex = if cfg!(debug_assertions) {
        // Lets tests and development builds use their own key.
        std::env::var("ARTISAN_RELEASE_PUBLIC_KEY_HEX")
            .unwrap_or_else(|_| PUBLIC_KEY_HEX.to_owned())
    } else {
        PUBLIC_KEY_HEX.to_owned()
    };
    let key: [u8; 32] = hex::decode(key_hex.trim())
        .context("invalid release public key hex")?
        .try_into()
        .map_err(|key: Vec<u8>| {
            anyhow::anyhow!("the release key is not 32 bytes but {}", key.len())
        })?;
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
    /// The version an app installs as, on `os`.
    pub fn version(&self, os: &str) -> Result<String> {
        match self {
            Self::Feed { manifest, .. } => Ok(manifest.version.clone()),
            Self::Local { dir } => {
                let path = dir.join(os).join("VERSION");
                let version = std::fs::read_to_string(&path)
                    .unwrap_or_else(|_| "0.0.0-local".to_owned())
                    .trim()
                    .to_owned();
                ensure!(
                    usable_name(&version),
                    "unusable version in {}",
                    path.display()
                );
                Ok(version)
            }
        }
    }

    pub const fn is_local(&self) -> bool {
        matches!(self, Self::Local { .. })
    }

    /// Whether this release has `app` for `os` on this CPU.
    pub fn has(&self, app: App, os: &str) -> bool {
        match self {
            Self::Feed { manifest, .. } => manifest
                .artifacts
                .iter()
                .any(|artifact| artifact.matches(app, os)),
            Self::Local { dir } => dir.join(os).join(app.binary_name_on(os)).is_file(),
        }
    }

    /// Puts `app`'s files for `os` (binary and notices) into `dest`, which
    /// must not exist: unpacked from the verified archive, or copied from the
    /// local build.
    pub async fn unpack(&self, app: App, os: &str, dest: &Path) -> Result<()> {
        match self {
            Self::Local { dir } => {
                let binary = app.binary_name_on(os);
                let from = dir.join(os).join(&binary);
                ensure!(from.is_file(), "the local build has no {}", from.display());
                std::fs::create_dir_all(dest)
                    .with_context(|| format!("create {}", dest.display()))?;
                std::fs::copy(&from, dest.join(&binary))
                    .with_context(|| format!("copy {}", from.display()))?;
                Ok(())
            }
            Self::Feed {
                manifest,
                source,
                http,
            } => {
                let artifact = manifest
                    .artifacts
                    .iter()
                    .find(|artifact| artifact.matches(app, os))
                    .with_context(|| {
                        format!(
                            "release {} has no {} for {os} {}",
                            manifest.version,
                            app.id(),
                            std::env::consts::ARCH
                        )
                    })?;
                ensure!(
                    usable_name(&artifact.file),
                    "unusable archive name {:?}",
                    artifact.file
                );
                let label = format!("Downloading {} {}", app.display_name(), manifest.version);
                let length = match source {
                    Source::Url(_) => http
                        .head(source.file(&artifact.file))
                        .send()
                        .await
                        .ok()
                        .and_then(|response| response.content_length()),
                    Source::Dir(_) => None,
                };
                let progress = length.map_or_else(
                    || Progress::spinner(&label),
                    |length| Progress::download(&label, length),
                );

                let bytes = match fetch(http, source, &artifact.file, Some(&progress)).await {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        progress.fail(format!("{error:#}"));
                        return Err(error);
                    }
                };
                let actual = hex::encode(Sha256::digest(&bytes));
                if !actual.eq_ignore_ascii_case(&artifact.sha256) {
                    let message = format!(
                        "{} is corrupt or was tampered with (SHA-256 {actual}, expected {})",
                        artifact.file, artifact.sha256
                    );
                    progress.fail(&message);
                    bail!(message);
                }
                progress.done(format!(
                    "Downloaded {} {}",
                    app.display_name(),
                    manifest.version
                ));
                extract(&artifact.file, &bytes, dest)
            }
        }
    }
}

impl Artifact {
    fn matches(&self, app: App, os: &str) -> bool {
        self.app == app.id() && self.os == os && self.arch == std::env::consts::ARCH
    }
}

fn extract(file_name: &str, bytes: &[u8], dest: &Path) -> Result<()> {
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

/// Whether `candidate` is newer than `installed` (`ae update`).
pub fn newer(candidate: &str, installed: &str) -> bool {
    match (
        semver::Version::parse(candidate),
        semver::Version::parse(installed),
    ) {
        (Ok(candidate), Ok(installed)) => candidate.cmp_precedence(&installed).is_gt(),
        _ => candidate != installed,
    }
}
