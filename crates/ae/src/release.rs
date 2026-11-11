use crate::bundle;
use crate::ui::Progress;
use anyhow::{Context, Result, bail, ensure};
use artisan_paths::App;
use base64::Engine as _;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub const DEFAULT_FEED: &str =
    "https://github.com/artisanstreet/editor/releases/latest/download/release.json";
pub const NIGHTLY_FEED: &str =
    "https://github.com/artisanstreet/editor/releases/download/nightly/release.json";
const MANIFEST: &str = "release.json";

const PUBLIC_KEY_HEX: &str = match option_env!("ARTISAN_RELEASE_PUBLIC_KEY_HEX") {
    Some(key) => key,
    None => "400a4b35cffdfa088227b9824176affb37b807fb26eb3c59b48ae56fbf5e4d8b",
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

pub fn feed_for(version: &str) -> &'static str {
    match semver::Version::parse(version) {
        Ok(version) if version.pre.as_str().starts_with("nightly") => NIGHTLY_FEED,
        _ => DEFAULT_FEED,
    }
}

pub fn carrier(os: &str) -> App {
    if os == "windows" {
        App::Editor
    } else {
        App::Ae
    }
}

#[derive(Clone, Debug)]
pub enum Source {
    Bundled,
    Url(String),
    Dir(PathBuf),
}

impl Source {
    pub fn new(from: Option<&str>, version: Option<&str>, bundled: bool) -> Result<Self> {
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
            (None, None) if bundled || bundle::FORGE.present() => Self::Bundled,
            (None, None) => Self::Url(feed_for(env!("CARGO_PKG_VERSION")).to_owned()),
        })
    }

    pub fn argument(&self) -> String {
        match self {
            Self::Bundled => "this installer".to_owned(),
            Self::Url(url) => url.clone(),
            Self::Dir(dir) => dir.display().to_string(),
        }
    }

    pub fn update_feed(&self) -> Option<&str> {
        match self {
            Self::Url(url)
                if url
                    .strip_prefix("https://github.com/artisanstreet/editor/releases/download/v")
                    .and_then(|version| version.strip_suffix("/release.json"))
                    .and_then(|version| semver::Version::parse(version).ok())
                    .is_some_and(|version| version.pre.is_empty()) =>
            {
                Some(DEFAULT_FEED)
            }
            Self::Url(url) => Some(url),
            Self::Bundled => Some(feed_for(env!("CARGO_PKG_VERSION"))),
            Self::Dir(_) => None,
        }
    }

    fn file(&self, name: &str) -> String {
        match self {
            Self::Bundled => name.to_owned(),
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

#[derive(Clone, Debug, Default)]
pub struct Carried {
    pub editor: Option<PathBuf>,
    pub linux_ae: Option<PathBuf>,
}

pub struct Feed {
    manifest: Manifest,
    source: Source,
    http: reqwest::Client,
}

pub enum Release {
    Feed(Feed),
    Local { dir: PathBuf },
    Bundled(Carried),
}

pub async fn resolve(source: &Source, carried: Carried) -> Result<Release> {
    match source {
        Source::Bundled => return Ok(Release::Bundled(carried)),
        Source::Dir(dir) if !dir.join(MANIFEST).is_file() => {
            ensure!(
                dir.is_dir(),
                "{} is neither a release nor a local build",
                dir.display()
            );
            return Ok(Release::Local { dir: dir.clone() });
        }
        Source::Dir(_) | Source::Url(_) => {}
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
    Ok(Release::Feed(Feed {
        manifest,
        source: source.clone(),
        http,
    }))
}

async fn fetch(
    http: &reqwest::Client,
    source: &Source,
    name: &str,
    progress: Option<&Progress>,
) -> Result<Vec<u8>> {
    let location = source.file(name);
    match source {
        Source::Bundled => bail!("this installer carries no {name}"),
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

impl Feed {
    pub fn version(&self) -> &str {
        &self.manifest.version
    }

    pub fn update_feed(&self) -> Option<&str> {
        self.source.update_feed()
    }

    pub fn carries(&self, os: &str) -> bool {
        self.artifact(os).is_some()
    }

    fn artifact(&self, os: &str) -> Option<&Artifact> {
        let app = carrier(os);
        self.manifest.artifacts.iter().find(|artifact| {
            artifact.app == app.id() && artifact.os == os && artifact.arch == std::env::consts::ARCH
        })
    }

    pub async fn fetch_carrier(&self, os: &str, folder: &Path) -> Result<PathBuf> {
        let artifact = self.artifact(os).with_context(|| {
            format!(
                "release {} has no installer for {os} {}",
                self.manifest.version,
                std::env::consts::ARCH
            )
        })?;
        ensure!(
            usable_name(&artifact.file),
            "unusable file name {:?}",
            artifact.file
        );
        let label = format!("Downloading Artisan {}", self.manifest.version);
        let length = match &self.source {
            Source::Url(_) => self
                .http
                .head(self.source.file(&artifact.file))
                .send()
                .await
                .ok()
                .and_then(|response| response.content_length()),
            Source::Dir(_) | Source::Bundled => None,
        };
        let progress = length.map_or_else(
            || Progress::spinner(&label),
            |length| Progress::download(&label, length),
        );
        let bytes = match fetch(&self.http, &self.source, &artifact.file, Some(&progress)).await {
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
        std::fs::create_dir_all(folder).with_context(|| format!("create {}", folder.display()))?;
        let path = folder.join(&artifact.file);
        std::fs::write(&path, &bytes).with_context(|| format!("write {}", path.display()))?;
        make_runnable(&path)?;
        progress.done(format!("Downloaded Artisan {}", self.manifest.version));
        Ok(path)
    }
}

#[cfg_attr(
    not(unix),
    expect(
        clippy::missing_const_for_fn,
        clippy::unnecessary_wraps,
        reason = "only Unix marks a file runnable"
    )
)]
fn make_runnable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
            .with_context(|| format!("make {} runnable", path.display()))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

impl Release {
    pub fn version(&self, os: &str) -> Result<String> {
        match self {
            Self::Feed(feed) => Ok(feed.manifest.version.clone()),
            Self::Bundled(_) => Ok(env!("CARGO_PKG_VERSION").to_owned()),
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

    pub fn has(&self, app: App, os: &str) -> bool {
        let here = os == std::env::consts::OS;
        let into_wsl = cfg!(windows) && os == "linux";
        match self {
            Self::Local { dir } => dir.join(os).join(app.binary_name_on(os)).is_file(),
            Self::Feed(feed) => match app {
                App::Editor => os == "windows" && feed.carries("windows"),
                App::Ae | App::Forge => {
                    (here && feed.carries(os)) || (into_wsl && feed.carries("windows"))
                }
            },
            Self::Bundled(carried) => match app {
                App::Editor => here && carried.editor.is_some(),
                App::Ae => here || (into_wsl && carried.linux_ae.is_some()),
                App::Forge => {
                    (here && bundle::FORGE.present()) || (into_wsl && carried.linux_ae.is_some())
                }
            },
        }
    }

    pub fn unpack(&self, app: App, os: &str, dest: &Path) -> Result<()> {
        let binary = app.binary_name_on(os);
        let here = os == std::env::consts::OS;
        std::fs::create_dir_all(dest).with_context(|| format!("create {}", dest.display()))?;
        let target = dest.join(&binary);
        let copy = |from: &Path| -> Result<()> {
            ensure!(from.is_file(), "there is no {}", from.display());
            std::fs::copy(from, &target).with_context(|| format!("copy {}", from.display()))?;
            make_runnable(&target)
        };
        match self {
            Self::Feed(_) => bail!("a release feed installs through its own installer"),
            Self::Local { dir } => copy(&dir.join(os).join(&binary)),
            Self::Bundled(carried) => match (app, here) {
                (App::Ae, true) => copy(&std::env::current_exe().context("find this ae")?),
                (App::Ae, false) => copy(
                    carried
                        .linux_ae
                        .as_deref()
                        .context("this installer carries no ae for Linux")?,
                ),
                (App::Forge, true) => bundle::FORGE
                    .write_program(&target)
                    .with_context(|| format!("write {}", target.display())),
                (App::Editor, true) => copy(
                    carried
                        .editor
                        .as_deref()
                        .context("this installer carries no Editor")?,
                ),
                (App::Forge | App::Editor, false) => {
                    bail!("this installer carries no {} for {os}", app.display_name())
                }
            },
        }
    }
}

pub fn usable_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b".-_+".contains(&byte))
}

pub fn newer(candidate: &str, installed: &str) -> bool {
    match (
        semver::Version::parse(candidate),
        semver::Version::parse(installed),
    ) {
        (Ok(candidate), Ok(installed)) => candidate.cmp_precedence(&installed).is_gt(),
        _ => candidate != installed,
    }
}

#[cfg(test)]
#[path = "../tests/release/feed.rs"]
mod tests;
