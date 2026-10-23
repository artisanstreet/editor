//! Engine installs under `<data>/engines/<engine>/`:
//!
//! ```text
//! versions/<version>/…   the unpacked engine (current and one previous)
//! current, previous      one version per file
//! home/                  the engine's HOME
//! install.lock           held while installing or switching
//! ```
//!
//! `ae` and Forge can both install, so every change happens under the
//! engine's file lock. Downloads land in `versions/` and are renamed into
//! place, so a crash never leaves a half-written version as `current`.

use crate::engines::{
    Archive, Artifact, CLAUDE_RELEASES, Feed, NPM_REGISTRY, compare_versions, host_spec,
};
use anyhow::{Context, Result, bail, ensure};
use artisan_api::{EngineId, InstallState};
use base64::Engine as _;
use sha2::Digest as _;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use tokio::io::AsyncWriteExt;

/// Largest download we accept.
const MAX_DOWNLOAD_BYTES: u64 = 1024 * 1024 * 1024;
const PROGRESS_STEP_BYTES: u64 = 2 * 1024 * 1024;

// ---------------------------------------------------------------- machine

#[derive(Debug)]
pub enum Event {
    /// What `current` says on disk (at start, or after another process changed it).
    Loaded {
        version: Option<String>,
    },
    Start {
        version: String,
    },
    Progress {
        received: u64,
        total: Option<u64>,
    },
    Downloaded,
    Done {
        version: String,
    },
    Fail {
        message: String,
    },
    UpdateAvailable {
        version: String,
    },
}

/// The install lifecycle retains the old serving version during a replacement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Absent {},
    Installed {
        version: String,
        update: Option<String>,
    },
    Downloading {
        version: String,
        received: u64,
        total: Option<u64>,
        serving: Option<String>,
    },
    Verifying {
        version: String,
        serving: Option<String>,
    },
    Failed {
        message: String,
        serving: Option<String>,
    },
}

pub struct Machine {
    state: State,
}

impl Machine {
    pub(crate) const fn state(&self) -> &State {
        &self.state
    }

    pub(crate) fn handle(&mut self, event: &Event) {
        let next = match (&mut self.state, event) {
            (State::Absent {}, Event::Start { version }) => Some(State::Downloading {
                version: version.clone(),
                received: 0,
                total: None,
                serving: None,
            }),
            (State::Absent {}, Event::Fail { message }) => Some(State::Failed {
                message: message.clone(),
                serving: None,
            }),
            (State::Installed { .. }, Event::Loaded { version: None }) => Some(State::Absent {}),
            (
                State::Installed { version, .. },
                Event::Loaded {
                    version: Some(found),
                }
                | Event::Done { version: found },
            ) if found != version => Some(State::Installed {
                version: found.clone(),
                update: None,
            }),
            (State::Installed { version, .. }, Event::Start { version: target }) => {
                Some(State::Downloading {
                    version: target.clone(),
                    received: 0,
                    total: None,
                    serving: Some(version.clone()),
                })
            }
            (State::Installed { version, .. }, Event::Fail { message }) => Some(State::Failed {
                message: message.clone(),
                serving: Some(version.clone()),
            }),
            (State::Installed { version, update }, Event::UpdateAvailable { version: newer }) => {
                *update = (newer != version).then(|| newer.clone());
                None
            }
            (
                State::Downloading {
                    received, total, ..
                },
                Event::Progress {
                    received: now,
                    total: of,
                },
            ) => {
                *received = *now;
                *total = *of;
                None
            }
            (
                State::Downloading {
                    version, serving, ..
                },
                Event::Downloaded,
            ) => Some(State::Verifying {
                version: version.clone(),
                serving: serving.clone(),
            }),
            (
                State::Absent {},
                Event::Loaded {
                    version: Some(version),
                }
                | Event::Done { version },
            )
            | (
                State::Downloading { .. } | State::Verifying { .. } | State::Failed { .. },
                Event::Done { version },
            )
            | (
                State::Failed { .. },
                Event::Loaded {
                    version: Some(version),
                },
            ) => Some(State::Installed {
                version: version.clone(),
                update: None,
            }),
            (
                State::Downloading { serving, .. } | State::Verifying { serving, .. },
                Event::Fail { message },
            ) => Some(State::Failed {
                message: message.clone(),
                serving: serving.clone(),
            }),
            (State::Failed { serving, .. }, Event::Start { version }) => Some(State::Downloading {
                version: version.clone(),
                received: 0,
                total: None,
                serving: serving.clone(),
            }),
            _ => None,
        };
        if let Some(next) = next {
            self.state = next;
        }
    }
}

pub const fn machine() -> Machine {
    Machine {
        state: State::Absent {},
    }
}

pub fn view(state: &State) -> InstallState {
    match state {
        State::Absent {} => InstallState::Absent,
        State::Downloading {
            version,
            received,
            total,
            ..
        } => InstallState::Downloading {
            version: version.clone(),
            received: *received,
            total: *total,
        },
        State::Verifying { version, .. } => InstallState::Verifying {
            version: version.clone(),
        },
        State::Installed { version, update } => InstallState::Installed {
            version: version.clone(),
            update: update.clone(),
        },
        State::Failed { message, serving } => InstallState::Failed {
            message: serving.as_ref().map_or_else(
                || message.clone(),
                |version| format!("{message} (still using {version})"),
            ),
        },
    }
}

/// Whether the machine is between installs, so the disk is the truth.
pub const fn settled(state: &State) -> bool {
    matches!(
        state,
        State::Absent {} | State::Installed { .. } | State::Failed { .. }
    )
}

// ---------------------------------------------------------------- layout

#[derive(Clone, Debug)]
pub struct Layout {
    root: PathBuf,
}

impl Layout {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            root: data_dir.join("engines"),
        }
    }

    pub fn engine_dir(&self, engine: EngineId) -> PathBuf {
        self.root.join(engine.as_str())
    }

    pub fn versions(&self, engine: EngineId) -> PathBuf {
        self.engine_dir(engine).join("versions")
    }

    pub fn version_dir(&self, engine: EngineId, version: &str) -> PathBuf {
        self.versions(engine).join(version)
    }

    pub fn home(&self, engine: EngineId) -> PathBuf {
        self.engine_dir(engine).join("home")
    }

    pub fn current(&self, engine: EngineId) -> Option<String> {
        read_pointer(&self.engine_dir(engine).join("current"))
    }

    pub fn previous(&self, engine: EngineId) -> Option<String> {
        read_pointer(&self.engine_dir(engine).join("previous"))
    }

    /// The executable of `version`, if that version is unpacked here.
    pub fn executable(&self, engine: EngineId, version: &str) -> Option<PathBuf> {
        let entry = host_spec(engine).artifact?.entry;
        let path = self.version_dir(engine, version).join(entry);
        path.is_file().then_some(path)
    }

    pub fn binary(&self, engine: EngineId) -> Option<PathBuf> {
        self.executable(engine, &self.current(engine)?)
    }
}

fn read_pointer(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let version = text.trim();
    check_version(version).ok()?;
    Some(version.to_owned())
}

/// Versions come from vendor feeds and become directory names.
pub fn check_version(version: &str) -> Result<()> {
    ensure!(
        !version.is_empty()
            && version.len() <= 64
            && !version.starts_with('.')
            && version
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b".-_+".contains(&byte)),
        "unusable version {version:?}"
    );
    Ok(())
}

fn write_pointer(path: &Path, version: &str) -> Result<()> {
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, format!("{version}\n"))
        .with_context(|| format!("writing {}", temporary.display()))?;
    fs::rename(&temporary, path).with_context(|| format!("replacing {}", path.display()))
}

/// Makes `version` current and keeps the old current as previous.
/// Pruning belongs to the manager, which knows versions leased by live runs.
pub fn switch(layout: &Layout, engine: EngineId, version: &str) -> Result<()> {
    let dir = layout.engine_dir(engine);
    let current = layout.current(engine);
    if current.as_deref() != Some(version) {
        if let Some(old) = &current {
            write_pointer(&dir.join("previous"), old)?;
        }
        write_pointer(&dir.join("current"), version)?;
        tracing::info!(%engine, version, previous = current.as_deref(), "engine version switched");
    }
    Ok(())
}

/// One standard lock per engine; active use keeps all installed versions available.
pub fn usage_lock(home: &Path) -> Result<fs::File> {
    let dir = home.parent().context("engine home has no parent")?;
    fs::create_dir_all(dir)?;
    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("usage.lock"))
        .context("opening engine usage lock")
}

pub fn prune(layout: &Layout, engine: EngineId) {
    let Ok(usage) = usage_lock(&layout.home(engine)) else {
        return;
    };
    if fs2::FileExt::try_lock_exclusive(&usage).is_err() {
        return;
    }
    let keep = [layout.current(engine), layout.previous(engine)];
    let Ok(entries) = fs::read_dir(layout.versions(engine)) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if keep
            .iter()
            .flatten()
            .any(|version| name == version.as_str())
        {
            continue;
        }
        let path = entry.path();
        let removed = if path.is_dir() {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_file(&path)
        };
        // A running engine can pin its files on Windows; the next prune retries.
        if let Err(error) = removed {
            tracing::warn!(%engine, path = %path.display(), %error, "could not remove an old engine version");
        }
    }
}

/// The engine's install lock, held until the returned file is dropped.
pub async fn lock(layout: &Layout, engine: EngineId) -> Result<fs::File> {
    let dir = layout.engine_dir(engine);
    tokio::task::spawn_blocking(move || {
        fs::create_dir_all(dir.join("versions"))
            .with_context(|| format!("creating {}", dir.display()))?;
        let path = dir.join("install.lock");
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .with_context(|| format!("opening {}", path.display()))?;
        fs2::FileExt::lock_exclusive(&file)
            .with_context(|| format!("locking {}", path.display()))?;
        Ok(file)
    })
    .await
    .context("install lock task")?
}

// ---------------------------------------------------------------- feeds

/// One downloadable version and the digest the vendor publishes for it.
#[derive(Clone, Debug)]
pub struct Resolved {
    pub version: String,
    pub url: String,
    pub digest: Expected,
}

#[derive(Clone, Debug)]
pub enum Expected {
    Sha256 { hex: String, size: u64 },
    Sha512(Vec<u8>),
}

async fn get(http: &reqwest::Client, url: &str) -> Result<reqwest::Response> {
    http.get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .with_context(|| format!("fetching {url}"))
}

async fn get_text(http: &reqwest::Client, url: &str) -> Result<String> {
    get(http, url)
        .await?
        .text()
        .await
        .with_context(|| format!("reading {url}"))
}

async fn get_json(http: &reqwest::Client, url: &str) -> Result<serde_json::Value> {
    let text = get_text(http, url).await?;
    serde_json::from_str(&text).with_context(|| format!("parsing {url}"))
}

fn npm_name(package: &str) -> String {
    package.replace('/', "%2f")
}

pub async fn latest_version(http: &reqwest::Client, feed: Feed) -> Result<String> {
    let version = match feed {
        Feed::Claude { .. } => get_text(http, &format!("{CLAUDE_RELEASES}/latest"))
            .await?
            .trim()
            .to_owned(),
        Feed::Npm {
            package, dist_tag, ..
        } => {
            let url = format!("{NPM_REGISTRY}/-/package/{}/dist-tags", npm_name(package));
            let tags = get_json(http, &url).await?;
            tags.get(dist_tag)
                .and_then(serde_json::Value::as_str)
                .with_context(|| format!("{package} has no {dist_tag:?} tag"))?
                .to_owned()
        }
        Feed::Release { repository, .. } => {
            let url =
                format!("https://github.com/{repository}/releases/latest/download/manifest.json");
            get_json(http, &url)
                .await?
                .get("version")
                .and_then(serde_json::Value::as_str)
                .with_context(|| format!("{url} names no version"))?
                .to_owned()
        }
    };
    check_version(&version)?;
    Ok(version)
}

/// The npm package Claude Code also publishes; its version list is the
/// release list (`downloads.claude.ai` has none).
const CLAUDE_NPM_PACKAGE: &str = "@anthropic-ai/claude-code";
/// The most versions "Choose version" offers.
pub const MAX_LISTED_VERSIONS: usize = 40;

/// The vendor's published versions on the channel Forge follows, newest
/// first.
pub async fn published_versions(http: &reqwest::Client, feed: Feed) -> Result<Vec<String>> {
    let (package, channel) = match feed {
        Feed::Claude { .. } => (CLAUDE_NPM_PACKAGE, "latest"),
        Feed::Npm {
            package, dist_tag, ..
        } => (package, dist_tag),
        Feed::Release { repository, .. } => return release_versions(http, repository).await,
    };
    let url = format!("{NPM_REGISTRY}/{}", npm_name(package));
    // The abbreviated document: versions without their readmes.
    let text = http
        .get(&url)
        .header(
            reqwest::header::ACCEPT,
            "application/vnd.npm.install-v1+json",
        )
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .with_context(|| format!("fetching {url}"))?
        .text()
        .await
        .with_context(|| format!("reading {url}"))?;
    let packument: serde_json::Value =
        serde_json::from_str(&text).with_context(|| format!("parsing {url}"))?;
    Ok(channel_versions(&packument, channel))
}

/// The versions a release feed has published, newest first.
async fn release_versions(http: &reqwest::Client, repository: &str) -> Result<Vec<String>> {
    let url = format!(
        "https://api.github.com/repos/{repository}/releases?per_page={MAX_LISTED_VERSIONS}"
    );
    Ok(tagged_versions(&get_json(http, &url).await?))
}

/// The `artisan-v<version>` releases in a GitHub release listing; drafts and
/// other tags are left out.
pub fn tagged_versions(releases: &serde_json::Value) -> Vec<String> {
    let mut versions: Vec<String> = releases
        .as_array()
        .into_iter()
        .flatten()
        .filter(|release| release.get("draft").and_then(serde_json::Value::as_bool) != Some(true))
        .filter_map(|release| release.get("tag_name")?.as_str()?.strip_prefix(RELEASE_TAG))
        .filter(|version| check_version(version).is_ok())
        .map(str::to_owned)
        .collect();
    versions.sort_by(|left, right| compare_versions(right, left));
    versions.dedup();
    versions.truncate(MAX_LISTED_VERSIONS);
    versions
}

/// Release feeds tag each version `artisan-v<version>`.
const RELEASE_TAG: &str = "artisan-v";

/// The packument's versions on `channel`: plain `X.Y.Z` releases for
/// `latest`, `X.Y.Z-<channel>-<n>` builds for another tag (OpenCode's
/// `beta`). Platform builds (`0.162.1-linux-x64`) and other channels are
/// left out.
pub fn channel_versions(packument: &serde_json::Value, channel: &str) -> Vec<String> {
    let on_channel = |version: &str| match version.split_once('-') {
        None => channel == "latest",
        Some((_, suffix)) => {
            channel != "latest"
                && suffix
                    .strip_prefix(channel)
                    .and_then(|rest| rest.strip_prefix('-'))
                    .is_some_and(|number| {
                        !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
                    })
        }
    };
    let mut versions: Vec<String> = packument
        .get("versions")
        .and_then(serde_json::Value::as_object)
        .into_iter()
        .flat_map(serde_json::Map::keys)
        .filter(|version| check_version(version).is_ok() && on_channel(version))
        .cloned()
        .collect();
    versions.sort_by(|left, right| compare_versions(right, left));
    versions.dedup();
    versions.truncate(MAX_LISTED_VERSIONS);
    versions
}

pub async fn resolve(http: &reqwest::Client, feed: Feed, version: &str) -> Result<Resolved> {
    check_version(version)?;
    let (url, digest) = match feed {
        Feed::Claude { platform, binary } => {
            let manifest =
                get_json(http, &format!("{CLAUDE_RELEASES}/{version}/manifest.json")).await?;
            ensure!(
                manifest.get("version").and_then(serde_json::Value::as_str) == Some(version),
                "the Claude manifest for {version} names another version"
            );
            let entry = manifest
                .pointer(&format!("/platforms/{platform}"))
                .with_context(|| format!("Claude {version} has no {platform} build"))?;
            let hex = entry
                .get("checksum")
                .and_then(serde_json::Value::as_str)
                .context("no checksum")?;
            let size = entry
                .get("size")
                .and_then(serde_json::Value::as_u64)
                .context("no size")?;
            (
                format!("{CLAUDE_RELEASES}/{version}/{platform}/{binary}"),
                Expected::Sha256 {
                    hex: hex.to_ascii_lowercase(),
                    size,
                },
            )
        }
        Feed::Npm {
            package, suffix, ..
        } => {
            let document = get_json(
                http,
                &format!("{NPM_REGISTRY}/{}/{version}{suffix}", npm_name(package)),
            )
            .await?;
            let tarball = document
                .pointer("/dist/tarball")
                .and_then(serde_json::Value::as_str)
                .context("no tarball")?;
            ensure!(
                tarball.starts_with(&format!("{NPM_REGISTRY}/{package}/-/")),
                "{package} {version} points outside the registry: {tarball}"
            );
            let integrity = document
                .pointer("/dist/integrity")
                .and_then(serde_json::Value::as_str)
                .and_then(|value| value.strip_prefix("sha512-"))
                .context("no sha512 integrity")?;
            let digest = base64::engine::general_purpose::STANDARD
                .decode(integrity)
                .context("decoding the npm integrity")?;
            (tarball.to_owned(), Expected::Sha512(digest))
        }
        Feed::Release {
            repository,
            platform,
        } => resolve_release(http, repository, platform, version).await?,
    };
    Ok(Resolved {
        version: version.to_owned(),
        url,
        digest,
    })
}

/// The asset and digest a release feed's manifest lists for `platform`.
async fn resolve_release(
    http: &reqwest::Client,
    repository: &str,
    platform: &str,
    version: &str,
) -> Result<(String, Expected)> {
    let release =
        format!("https://github.com/{repository}/releases/download/{RELEASE_TAG}{version}");
    let manifest = get_json(http, &format!("{release}/manifest.json")).await?;
    ensure!(
        manifest.get("version").and_then(serde_json::Value::as_str) == Some(version),
        "the {repository} manifest for {version} names another version"
    );
    let entry = manifest
        .pointer(&format!("/platforms/{platform}"))
        .with_context(|| format!("{repository} {version} has no {platform} build"))?;
    let asset = entry
        .get("asset")
        .and_then(serde_json::Value::as_str)
        .filter(|asset| {
            !asset.is_empty()
                && asset
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b".-_".contains(&byte))
        })
        .context("no usable asset name")?;
    let hex = entry
        .get("sha256")
        .and_then(serde_json::Value::as_str)
        .context("no sha256")?;
    let size = entry
        .get("size")
        .and_then(serde_json::Value::as_u64)
        .context("no size")?;
    Ok((
        format!("{release}/{asset}"),
        Expected::Sha256 {
            hex: hex.to_ascii_lowercase(),
            size,
        },
    ))
}

// ---------------------------------------------------------------- download and unpack

enum Hasher {
    Sha256(sha2::Sha256),
    Sha512(sha2::Sha512),
}

/// Downloads, verifies and unpacks `resolved` into `versions/<version>`.
/// Call with the install lock held; `report` sees the lifecycle events.
pub async fn fetch(
    layout: &Layout,
    http: &reqwest::Client,
    engine: EngineId,
    artifact: Artifact,
    resolved: &Resolved,
    report: &(dyn Fn(Event) + Send + Sync),
) -> Result<()> {
    let version = &resolved.version;
    check_version(version)?;
    let versions = layout.versions(engine);
    let download = versions.join(format!(".download-{version}"));
    let staging = versions.join(format!(".staging-{version}"));
    report(Event::Start {
        version: version.clone(),
    });
    let result = async {
        let (size, digest) =
            download_to(http, &resolved.url, &download, &resolved.digest, report).await?;
        report(Event::Downloaded);
        verify(&resolved.digest, size, &digest)
            .with_context(|| format!("verifying {}", resolved.url))?;
        let (from, to, target) = (
            download.clone(),
            staging.clone(),
            layout.version_dir(engine, version),
        );
        let switching = layout.clone();
        let switching_version = version.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            unpack(artifact, &from, &to)?;
            if target.exists() {
                fs::remove_dir_all(&target)
                    .with_context(|| format!("removing {}", target.display()))?;
            }
            fs::rename(&to, &target)
                .with_context(|| format!("moving {} into place", target.display()))?;
            switch(&switching, engine, &switching_version)
        })
        .await
        .context("unpack task")?
    }
    .await;
    // Leftovers of this attempt; either may not exist.
    fs::remove_file(&download).ok();
    fs::remove_dir_all(&staging).ok();
    result.with_context(|| format!("installing {} {version}", engine.display_name()))
}

async fn download_to(
    http: &reqwest::Client,
    url: &str,
    path: &Path,
    expected: &Expected,
    report: &(dyn Fn(Event) + Send + Sync),
) -> Result<(u64, Vec<u8>)> {
    let mut response = get(http, url).await?;
    let total = response.content_length();
    if let Some(total) = total {
        ensure!(
            total <= MAX_DOWNLOAD_BYTES,
            "{url} is {total} bytes, more than we accept"
        );
    }
    let mut file = tokio::fs::File::create(path)
        .await
        .with_context(|| format!("creating {}", path.display()))?;
    let mut hasher = match expected {
        Expected::Sha256 { .. } => Hasher::Sha256(sha2::Sha256::new()),
        Expected::Sha512(_) => Hasher::Sha512(sha2::Sha512::new()),
    };
    let (mut received, mut reported) = (0_u64, 0_u64);
    report(Event::Progress { received, total });
    while let Some(chunk) = response
        .chunk()
        .await
        .with_context(|| format!("downloading {url}"))?
    {
        received += chunk.len() as u64;
        ensure!(
            received <= MAX_DOWNLOAD_BYTES,
            "{url} is larger than we accept"
        );
        match &mut hasher {
            Hasher::Sha256(hasher) => hasher.update(&chunk),
            Hasher::Sha512(hasher) => hasher.update(&chunk),
        }
        file.write_all(&chunk)
            .await
            .with_context(|| format!("writing {}", path.display()))?;
        if received - reported >= PROGRESS_STEP_BYTES {
            reported = received;
            report(Event::Progress { received, total });
        }
    }
    file.flush().await?;
    file.sync_all()
        .await
        .with_context(|| format!("syncing {}", path.display()))?;
    report(Event::Progress { received, total });
    let digest = match hasher {
        Hasher::Sha256(hasher) => hasher.finalize().to_vec(),
        Hasher::Sha512(hasher) => hasher.finalize().to_vec(),
    };
    Ok((received, digest))
}

fn verify(expected: &Expected, size: u64, digest: &[u8]) -> Result<()> {
    match expected {
        Expected::Sha256 { hex, size: want } => {
            ensure!(
                size == *want,
                "size {size} does not match the published {want}"
            );
            let got = hex::encode(digest);
            ensure!(
                got == *hex,
                "SHA-256 {got} does not match the published {hex}"
            );
        }
        Expected::Sha512(want) => ensure!(
            digest == want.as_slice(),
            "SHA-512 does not match the published integrity"
        ),
    }
    Ok(())
}

/// Unpacks the download into `dest` and checks the executable is there.
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "every other tar entry type (devices, fifos, GNU and PAX headers) is skipped"
)]
fn unpack(artifact: Artifact, download: &Path, dest: &Path) -> Result<()> {
    if dest.exists() {
        fs::remove_dir_all(dest)?;
    }
    fs::create_dir_all(dest).with_context(|| format!("creating {}", dest.display()))?;
    match artifact.archive {
        Archive::Binary => {
            let to = dest.join(artifact.entry);
            fs::rename(download, &to).or_else(|_| fs::copy(download, &to).map(drop))?;
        }
        Archive::TarGz => {
            let file = fs::File::open(download)?;
            let mut archive =
                tar::Archive::new(flate2::read::GzDecoder::new(io::BufReader::new(file)));
            for entry in archive.entries().context("reading the archive")? {
                let mut entry = entry.context("reading the archive")?;
                let path = entry.path()?.into_owned();
                let Some(relative) = inside(&path, artifact.strip)? else {
                    continue;
                };
                let to = dest.join(&relative);
                match entry.header().entry_type() {
                    tar::EntryType::Directory => fs::create_dir_all(&to)?,
                    tar::EntryType::Regular | tar::EntryType::Continuous => {
                        if let Some(parent) = to.parent() {
                            fs::create_dir_all(parent)?;
                        }
                        entry
                            .unpack(&to)
                            .with_context(|| format!("unpacking {}", relative.display()))?;
                    }
                    kind if kind.is_symlink() || kind.is_hard_link() => {
                        bail!("the archive links {} elsewhere", path.display())
                    }
                    _ => {}
                }
            }
        }
    }
    let executable = dest.join(artifact.entry);
    ensure!(
        executable.is_file(),
        "the download has no {}",
        artifact.entry
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&executable)?.permissions().mode();
        fs::set_permissions(&executable, fs::Permissions::from_mode(mode | 0o755))?;
    }
    Ok(())
}

/// The member's path below `strip`, `None` for members outside it.
fn inside(path: &Path, strip: &str) -> Result<Option<PathBuf>> {
    let Ok(relative) = path.strip_prefix(strip) else {
        return Ok(None);
    };
    if relative.as_os_str().is_empty() {
        return Ok(None);
    }
    ensure!(
        relative
            .components()
            .all(|part| matches!(part, Component::Normal(_))),
        "unsafe archive member {}",
        path.display()
    );
    Ok(Some(relative.to_path_buf()))
}

#[cfg(feature = "legacy-import")]
mod legacy;
#[cfg(feature = "legacy-import")]
pub(crate) use legacy::import_legacy;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Engines;
    use std::collections::HashMap;
    use tokio::io::AsyncReadExt as _;

    /// Serves `files` over plain HTTP on localhost; returns the base URL.
    async fn serve(files: HashMap<String, Vec<u8>>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut request = Vec::new();
                let mut buffer = [0_u8; 1024];
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let Ok(read) = socket.read(&mut buffer).await else {
                        break;
                    };
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..read]);
                }
                let request = String::from_utf8_lossy(&request);
                let path = request.split_whitespace().nth(1).unwrap_or_default();
                let response = files.get(path).map_or_else(
                    || b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n".to_vec(),
                    |body| {
                        [
                            format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n", body.len())
                                .into_bytes(),
                            body.clone(),
                        ]
                        .concat()
                    },
                );
                socket.write_all(&response).await.ok();
            }
        });
        base
    }

    fn script(version: &str) -> Vec<u8> {
        format!("#!/bin/sh\n# {version}\nexit 1\n").into_bytes()
    }

    fn tar_gz(entry: &str, version: &str) -> Vec<u8> {
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        for (path, body) in [
            ("package/package.json".to_owned(), b"{}".to_vec()),
            (format!("package/{entry}"), script(version)),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder
                .append_data(&mut header, path, body.as_slice())
                .unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    fn sha512(bytes: &[u8]) -> Expected {
        Expected::Sha512(sha2::Sha512::digest(bytes).to_vec())
    }

    fn versions_on_disk(data: &Path, engine: EngineId) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(Layout::new(data).versions(engine))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn an_independent_manager_cannot_prune_a_leased_binary() {
        let data = tempfile::tempdir().unwrap();
        let layout = Layout::new(data.path());
        let engine = EngineId::Codex;
        for version in ["0.150.0", "0.151.0", "0.152.0"] {
            let binary = layout
                .version_dir(engine, version)
                .join(host_spec(engine).artifact.unwrap().entry);
            fs::create_dir_all(binary.parent().unwrap()).unwrap();
            fs::write(binary, script(version)).unwrap();
        }
        switch(&layout, engine, "0.150.0").unwrap();
        let forge = Engines::new(data.path());
        let lease = forge.lease(engine).unwrap();
        switch(&layout, engine, "0.151.0").unwrap();
        switch(&layout, engine, "0.152.0").unwrap();
        let cli_layout = Layout::new(data.path());
        prune(&cli_layout, engine);
        assert!(
            lease.path().is_file(),
            "another owner pruned the active binary"
        );
        drop(lease);
        prune(&cli_layout, engine);
        assert_eq!(
            versions_on_disk(data.path(), engine),
            ["0.151.0", "0.152.0"]
        );
    }

    #[tokio::test]
    async fn updates_swap_current_and_keep_one_previous() {
        let data = tempfile::tempdir().unwrap();
        let engine = EngineId::Codex;
        let artifact = host_spec(engine).artifact.unwrap();
        let archives: Vec<(String, Vec<u8>)> = ["0.150.0", "0.151.0", "0.152.0"]
            .iter()
            .map(|v| ((*v).to_owned(), tar_gz(artifact.entry, v)))
            .collect();
        let base = serve(
            archives
                .iter()
                .map(|(v, bytes)| (format!("/{v}.tgz"), bytes.clone()))
                .collect(),
        )
        .await;
        let engines = Engines::new(data.path());
        assert_eq!(engines.status(engine).install, InstallState::Absent);
        let mut statuses = engines.subscribe();

        for (version, bytes) in &archives {
            let resolved = Resolved {
                version: version.clone(),
                url: format!("{base}/{version}.tgz"),
                digest: sha512(bytes),
            };
            engines
                .install_resolved(engine, artifact, resolved)
                .await
                .unwrap();
        }

        let mut seen = Vec::new();
        while let Ok(status) = statuses.try_recv() {
            let install = match status {
                crate::EngineChange::Full(status) => status.install,
                crate::EngineChange::Install { install, .. } => install,
            };
            seen.push(std::mem::discriminant(&install));
        }
        for expected in [
            InstallState::Downloading {
                version: String::new(),
                received: 0,
                total: None,
            },
            InstallState::Verifying {
                version: String::new(),
            },
            InstallState::Installed {
                version: String::new(),
                update: None,
            },
        ] {
            assert!(
                seen.contains(&std::mem::discriminant(&expected)),
                "never reported {expected:?}"
            );
        }
        assert_eq!(
            engines.status(engine).install,
            InstallState::Installed {
                version: "0.152.0".into(),
                update: None
            }
        );
        let binary = engines.binary(engine).unwrap();
        assert!(fs::read_to_string(&binary).unwrap().contains("0.152.0"));
        #[cfg(unix)]
        assert_ne!(
            std::os::unix::fs::PermissionsExt::mode(&fs::metadata(&binary).unwrap().permissions())
                & 0o111,
            0
        );
        assert_eq!(
            versions_on_disk(data.path(), engine),
            ["0.151.0", "0.152.0"]
        );

        assert_eq!(engines.rollback(engine).await.unwrap(), "0.151.0");
        assert!(
            fs::read_to_string(engines.binary(engine).unwrap())
                .unwrap()
                .contains("0.151.0")
        );
        assert_eq!(
            Layout::new(data.path()).previous(engine).as_deref(),
            Some("0.152.0")
        );
    }

    #[tokio::test]
    async fn a_download_that_fails_verification_leaves_the_old_version_current() {
        let data = tempfile::tempdir().unwrap();
        let engine = EngineId::Codex;
        let artifact = host_spec(engine).artifact.unwrap();
        let (good, bad) = (
            tar_gz(artifact.entry, "0.150.0"),
            tar_gz(artifact.entry, "0.151.0"),
        );
        let base = serve(HashMap::from([
            ("/good.tgz".to_owned(), good.clone()),
            ("/bad.tgz".to_owned(), bad),
        ]))
        .await;
        let engines = Engines::new(data.path());
        let install = |version: &str, path: &str, digest| Resolved {
            version: version.to_owned(),
            url: format!("{base}{path}"),
            digest,
        };
        engines
            .install_resolved(
                engine,
                artifact,
                install("0.150.0", "/good.tgz", sha512(&good)),
            )
            .await
            .unwrap();

        // The published digest is for other bytes.
        let error = engines
            .install_resolved(
                engine,
                artifact,
                install("0.151.0", "/bad.tgz", sha512(&good)),
            )
            .await
            .unwrap_err();
        assert!(format!("{error:#}").contains("SHA-512"), "{error:#}");

        let InstallState::Failed { message } = engines.status(engine).install else {
            panic!("not failed")
        };
        assert!(message.contains("still using 0.150.0"), "{message}");
        assert!(
            fs::read_to_string(engines.binary(engine).unwrap())
                .unwrap()
                .contains("0.150.0")
        );
        assert_eq!(versions_on_disk(data.path(), engine), ["0.150.0"]);
    }

    /// "Choose version" lists the channel Forge follows, newest first: the
    /// releases, not Codex's per-platform builds or alphas, and only
    /// OpenCode's numbered betas.
    #[test]
    fn published_versions_are_the_followed_channel_newest_first() {
        let mut codex = serde_json::Map::new();
        for minor in 100..160 {
            codex.insert(format!("0.{minor}.0"), serde_json::json!({}));
            codex.insert(format!("0.{minor}.0-linux-x64"), serde_json::json!({}));
        }
        codex.insert("0.160.0-alpha.2".into(), serde_json::json!({}));
        let versions = channel_versions(&serde_json::json!({ "versions": codex }), "latest");
        assert_eq!(versions.len(), MAX_LISTED_VERSIONS);
        assert_eq!(versions[0], "0.159.0");
        assert_eq!(versions[1], "0.158.0");
        assert!(versions.iter().all(|version| !version.contains('-')));

        let opencode = serde_json::json!({ "versions": {
            "0.0.0-beta-9999": {}, "0.0.0-beta-19271": {}, "1.18.18": {},
            "0.0.0-beta-19271-linux-x64": {}, "0.0.0-dev-1": {},
        } });
        assert_eq!(
            channel_versions(&opencode, "beta"),
            ["0.0.0-beta-19271", "0.0.0-beta-9999"]
        );
    }

    /// "Choose version" lists the release feed's own builds, newest first.
    #[test]
    fn a_release_feed_lists_its_tagged_builds() {
        let releases = serde_json::json!([
            { "tag_name": "artisan-v2.1.2", "draft": false },
            { "tag_name": "artisan-v2.1.10", "draft": false },
            { "tag_name": "artisan-v2.2.0", "draft": true },
            { "tag_name": "v1.19.3", "draft": false },
            { "tag_name": "artisan-v../2.1.1", "draft": false },
        ]);
        assert_eq!(tagged_versions(&releases), ["2.1.10", "2.1.2"]);
    }

    #[cfg(feature = "legacy-import")]
    #[tokio::test]
    async fn imports_the_old_toolchain_layout() {
        let old = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let opencode = old.path().join("opencode2");
        let entry = host_spec(EngineId::OpenCode).artifact.unwrap().entry;
        let binary = opencode.join("versions/generation-abc").join(entry);
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        fs::create_dir_all(opencode.join("versions/generation-old")).unwrap();
        fs::create_dir_all(opencode.join("home/data/opencode")).unwrap();
        fs::write(&binary, script("1.0.50")).unwrap();
        fs::write(opencode.join("home/data/opencode/auth.json"), "{}").unwrap();
        fs::write(
            opencode.join("state.json"),
            r#"{"active":{"binary":"opencode2","directory":"generation-abc","version":"1.0.50"},"format_version":2,
                "previous":[{"binary":"opencode2","directory":"generation-old","version":"1.0.46"}]}"#,
        )
        .unwrap();

        let engines = Engines::new(data.path());
        assert_eq!(
            engines.import_legacy(old.path()).await.unwrap(),
            [EngineId::OpenCode]
        );
        assert_eq!(
            engines.status(EngineId::OpenCode).install,
            InstallState::Installed {
                version: "1.0.50".into(),
                update: None
            }
        );
        assert!(
            fs::read_to_string(engines.binary(EngineId::OpenCode).unwrap())
                .unwrap()
                .contains("1.0.50")
        );
        assert_eq!(
            fs::read_to_string(
                engines
                    .home(EngineId::OpenCode)
                    .join("data/opencode/auth.json")
            )
            .unwrap(),
            "{}"
        );
        assert_eq!(
            engines.status(EngineId::Claude).install,
            InstallState::Absent
        );
        // Already imported: left alone.
        assert!(engines.import_legacy(old.path()).await.unwrap().is_empty());
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::{Event, State, machine};

    #[test]
    fn a_failed_replacement_keeps_the_serving_version_and_retry_replaces_it() {
        let mut machine = machine();
        machine.handle(&Event::Loaded {
            version: Some("1".into()),
        });
        machine.handle(&Event::Start {
            version: "2".into(),
        });
        // Disk probes and a stale second Start must not discard the active install.
        machine.handle(&Event::Loaded { version: None });
        machine.handle(&Event::Start {
            version: "3".into(),
        });
        machine.handle(&Event::Progress {
            received: 100,
            total: Some(200),
        });
        assert_eq!(
            machine.state(),
            &State::Downloading {
                version: "2".into(),
                received: 100,
                total: Some(200),
                serving: Some("1".into()),
            }
        );
        machine.handle(&Event::Downloaded);
        machine.handle(&Event::Fail {
            message: "checksum mismatch".into(),
        });
        assert_eq!(
            machine.state(),
            &State::Failed {
                message: "checksum mismatch".into(),
                serving: Some("1".into()),
            }
        );
        machine.handle(&Event::Start {
            version: "2".into(),
        });
        machine.handle(&Event::Done {
            version: "2".into(),
        });
        assert_eq!(
            machine.state(),
            &State::Installed {
                version: "2".into(),
                update: None
            }
        );
    }

    #[test]
    fn repeated_disk_or_completion_events_preserve_a_known_update() {
        let mut machine = machine();
        machine.handle(&Event::Loaded {
            version: Some("1".into()),
        });
        machine.handle(&Event::UpdateAvailable {
            version: "2".into(),
        });
        machine.handle(&Event::Done {
            version: "1".into(),
        });
        machine.handle(&Event::Loaded {
            version: Some("1".into()),
        });
        assert_eq!(
            machine.state(),
            &State::Installed {
                version: "1".into(),
                update: Some("2".into())
            }
        );
        machine.handle(&Event::Loaded {
            version: Some("2".into()),
        });
        assert_eq!(
            machine.state(),
            &State::Installed {
                version: "2".into(),
                update: None
            }
        );
        machine.handle(&Event::Loaded { version: None });
        assert_eq!(machine.state(), &State::Absent {});
    }
}
