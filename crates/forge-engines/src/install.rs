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
    Archive, Artifact, CLAUDE_RELEASES, CURSOR_INSTALLER, CURSOR_RELEASES, Feed, GROK_RELEASES,
    NPM_REGISTRY, compare_versions, host_spec,
};
use anyhow::{Context, Result, bail, ensure};
use artisan_api::{EngineId, InstallState};
use base64::Engine as _;
use sha2::Digest as _;
use statig::prelude::*;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use tokio::io::AsyncWriteExt;

/// Largest download we accept.
const MAX_DOWNLOAD_BYTES: u64 = 1024 * 1024 * 1024;
const PROGRESS_STEP_BYTES: u64 = 2 * 1024 * 1024;

// ---------------------------------------------------------------- machine

/// The install lifecycle of one engine. Holds only the version being
/// installed and the one still serving; the owner publishes the state.
#[derive(Default)]
pub(crate) struct Install;

#[derive(Debug)]
pub(crate) enum Event {
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

#[state_machine(initial = "State::absent()", state(derive(Debug, Clone, PartialEq)))]
impl Install {
    #[state]
    fn absent(event: &Event) -> Outcome<State> {
        match event {
            Event::Loaded {
                version: Some(version),
            }
            | Event::Done { version } => Transition(State::installed(version.clone(), None)),
            Event::Start { version } => {
                Transition(State::downloading(version.clone(), 0, None, None))
            }
            Event::Fail { message } => Transition(State::failed(message.clone(), None)),
            _ => Handled,
        }
    }

    #[state]
    fn installed(
        version: &mut String,
        update: &mut Option<String>,
        event: &Event,
    ) -> Outcome<State> {
        match event {
            Event::Loaded { version: None } => Transition(State::absent()),
            Event::Loaded {
                version: Some(found),
            }
            | Event::Done { version: found }
                if found != version =>
            {
                Transition(State::installed(found.clone(), None))
            }
            Event::Start { version: target } => Transition(State::downloading(
                target.clone(),
                0,
                None,
                Some(version.clone()),
            )),
            Event::Fail { message } => {
                Transition(State::failed(message.clone(), Some(version.clone())))
            }
            Event::UpdateAvailable { version: newer } => {
                *update = (newer != version).then(|| newer.clone());
                Handled
            }
            _ => Handled,
        }
    }

    #[state(superstate = "busy")]
    #[expect(
        clippy::ptr_arg,
        reason = "statig takes the state's field types from these parameters"
    )]
    fn downloading(
        version: &mut String,
        received: &mut u64,
        total: &mut Option<u64>,
        serving: &mut Option<String>,
        event: &Event,
    ) -> Outcome<State> {
        match event {
            Event::Progress {
                received: now,
                total: of,
            } => {
                *received = *now;
                *total = *of;
                Handled
            }
            Event::Downloaded => Transition(State::verifying(version.clone(), serving.clone())),
            _ => Super,
        }
    }

    #[state(superstate = "busy")]
    fn verifying(
        version: &mut String,
        serving: &mut Option<String>,
        event: &Event,
    ) -> Outcome<State> {
        let _ = (version, serving, event);
        Super
    }

    /// Downloading or verifying: the old version (if any) keeps serving.
    #[superstate]
    fn busy(serving: &mut Option<String>, event: &Event) -> Outcome<State> {
        match event {
            Event::Done { version } => Transition(State::installed(version.clone(), None)),
            Event::Fail { message } => Transition(State::failed(message.clone(), serving.clone())),
            // A second install waits on the file lock, so a Start here is stale.
            _ => Handled,
        }
    }

    #[state]
    fn failed(message: &mut String, serving: &mut Option<String>, event: &Event) -> Outcome<State> {
        let _ = message;
        match event {
            Event::Start { version } => Transition(State::downloading(
                version.clone(),
                0,
                None,
                serving.clone(),
            )),
            Event::Loaded {
                version: Some(version),
            }
            | Event::Done { version } => Transition(State::installed(version.clone(), None)),
            _ => Handled,
        }
    }
}

pub(crate) type Machine = statig::blocking::StateMachine<Install>;

pub(crate) fn machine() -> Machine {
    Install.state_machine()
}

pub(crate) fn view(state: &State) -> InstallState {
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
            message: match serving {
                Some(version) => format!("{message} (still using {version})"),
                None => message.clone(),
            },
        },
    }
}

/// Whether the machine is between installs, so the disk is the truth.
pub(crate) fn settled(state: &State) -> bool {
    matches!(
        state,
        State::Absent {} | State::Installed { .. } | State::Failed { .. }
    )
}

// ---------------------------------------------------------------- layout

#[derive(Clone, Debug)]
pub(crate) struct Layout {
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

    /// Every version unpacked here with its executable.
    pub fn installed_versions(&self, engine: EngineId) -> Vec<String> {
        let Ok(entries) = fs::read_dir(self.versions(engine)) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|version| check_version(version).is_ok())
            .filter(|version| self.executable(engine, version).is_some())
            .collect()
    }
}

fn read_pointer(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let version = text.trim();
    check_version(version).ok()?;
    Some(version.to_owned())
}

/// Versions come from vendor feeds and become directory names.
pub(crate) fn check_version(version: &str) -> Result<()> {
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

/// Makes `version` current, keeps the old current as previous and removes
/// every other version. Call with the install lock held.
pub(crate) fn switch(layout: &Layout, engine: EngineId, version: &str) -> Result<()> {
    let dir = layout.engine_dir(engine);
    let current = layout.current(engine);
    if current.as_deref() != Some(version) {
        if let Some(old) = &current {
            write_pointer(&dir.join("previous"), old)?;
        }
        write_pointer(&dir.join("current"), version)?;
        tracing::info!(%engine, version, previous = current.as_deref(), "engine version switched");
    }
    prune(layout, engine);
    Ok(())
}

fn prune(layout: &Layout, engine: EngineId) {
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
pub(crate) async fn lock(layout: &Layout, engine: EngineId) -> Result<fs::File> {
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
pub(crate) struct Resolved {
    pub version: String,
    pub url: String,
    pub digest: Expected,
}

#[derive(Clone, Debug)]
pub(crate) enum Expected {
    Sha256 {
        hex: String,
        size: u64,
    },
    Sha512(Vec<u8>),
    /// The vendor publishes no digest; HTTPS is all we have.
    Nothing,
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

pub(crate) async fn latest_version(http: &reqwest::Client, feed: Feed) -> Result<String> {
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
        Feed::Grok { .. } => get_text(http, &format!("{GROK_RELEASES}/stable"))
            .await?
            .trim()
            .to_owned(),
        Feed::Cursor { .. } => {
            // The official installer names its release in the URL it downloads.
            let script = get_text(http, CURSOR_INSTALLER).await?;
            let marker = format!("{CURSOR_RELEASES}/");
            let start = script
                .find(&marker)
                .context("the Cursor installer names no release")?
                + marker.len();
            script[start..]
                .split('/')
                .next()
                .unwrap_or_default()
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
pub(crate) const MAX_LISTED_VERSIONS: usize = 40;

/// The vendor's published versions on the channel Forge follows, newest
/// first. `None` when the vendor publishes no list (Grok, Cursor).
pub(crate) async fn published_versions(
    http: &reqwest::Client,
    feed: Feed,
) -> Result<Option<Vec<String>>> {
    let (package, channel) = match feed {
        Feed::Claude { .. } => (CLAUDE_NPM_PACKAGE, "latest"),
        Feed::Npm {
            package, dist_tag, ..
        } => (package, dist_tag),
        Feed::Grok { .. } | Feed::Cursor { .. } => return Ok(None),
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
    Ok(Some(channel_versions(&packument, channel)))
}

/// The packument's versions on `channel`: plain `X.Y.Z` releases for
/// `latest`, `X.Y.Z-<channel>-<n>` builds for another tag (OpenCode's
/// `beta`). Platform builds (`0.162.1-linux-x64`) and other channels are
/// left out.
pub(crate) fn channel_versions(packument: &serde_json::Value, channel: &str) -> Vec<String> {
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

pub(crate) async fn resolve(http: &reqwest::Client, feed: Feed, version: &str) -> Result<Resolved> {
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
        Feed::Grok { platform, exe } => {
            let suffix = if exe { ".exe" } else { "" };
            (
                format!("{GROK_RELEASES}/grok-{version}-{platform}{suffix}"),
                Expected::Nothing,
            )
        }
        Feed::Cursor { os, arch } => (
            format!("{CURSOR_RELEASES}/{version}/{os}/{arch}/agent-cli-package.tar.gz"),
            Expected::Nothing,
        ),
    };
    Ok(Resolved {
        version: version.to_owned(),
        url,
        digest,
    })
}

// ---------------------------------------------------------------- download and unpack

enum Hasher {
    Sha256(sha2::Sha256),
    Sha512(sha2::Sha512),
    Nothing,
}

/// Downloads, verifies and unpacks `resolved` into `versions/<version>`.
/// Call with the install lock held; `report` sees the lifecycle events.
pub(crate) async fn fetch(
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
        tokio::task::spawn_blocking(move || -> Result<()> {
            unpack(artifact, &from, &to)?;
            if target.exists() {
                fs::remove_dir_all(&target)
                    .with_context(|| format!("removing {}", target.display()))?;
            }
            fs::rename(&to, &target)
                .with_context(|| format!("moving {} into place", target.display()))
        })
        .await
        .context("unpack task")??;
        switch(layout, engine, version)
    }
    .await;
    let _ = fs::remove_file(&download);
    let _ = fs::remove_dir_all(&staging);
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
        Expected::Nothing => Hasher::Nothing,
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
            Hasher::Nothing => {}
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
        Hasher::Nothing => Vec::new(),
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
            let got: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
            ensure!(
                got == *hex,
                "SHA-256 {got} does not match the published {hex}"
            );
        }
        Expected::Sha512(want) => ensure!(
            digest == want.as_slice(),
            "SHA-512 does not match the published integrity"
        ),
        Expected::Nothing => {}
    }
    Ok(())
}

/// Unpacks the download into `dest` and checks the executable is there.
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
        Archive::Zip => {
            let mut archive =
                zip::ZipArchive::new(fs::File::open(download)?).context("reading the zip")?;
            for index in 0..archive.len() {
                let mut file = archive.by_index(index)?;
                let path = file
                    .enclosed_name()
                    .with_context(|| format!("unsafe zip member {:?}", file.name()))?;
                let Some(relative) = inside(&path, artifact.strip)? else {
                    continue;
                };
                let to = dest.join(&relative);
                if file.is_dir() {
                    fs::create_dir_all(&to)?;
                    continue;
                }
                if let Some(parent) = to.parent() {
                    fs::create_dir_all(parent)?;
                }
                io::copy(&mut file, &mut fs::File::create(&to)?)
                    .with_context(|| format!("unpacking {}", relative.display()))?;
                #[cfg(unix)]
                if let Some(mode) = file.unix_mode() {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(&to, fs::Permissions::from_mode(mode & 0o777))?;
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

// ---------------------------------------------------------------- legacy import

/// Brings engines installed by the old Forge (`<old data>/toolchain/<engine>/`)
/// into this layout: the active version and the home, hard-linked where the
/// filesystem allows. Engines that already have a `current` are left alone.
pub(crate) async fn import_legacy(layout: &Layout, old_toolchain: &Path) -> Result<Vec<EngineId>> {
    let mut imported = Vec::new();
    for engine in EngineId::ALL {
        let Some(artifact) = host_spec(engine).artifact else {
            continue;
        };
        let old = old_toolchain.join(host_spec(engine).legacy_dir);
        let Ok(state) = fs::read(old.join("state.json")) else {
            continue;
        };
        let _lock = lock(layout, engine).await?;
        if layout.current(engine).is_some() {
            continue;
        }
        let state: serde_json::Value = serde_json::from_slice(&state)
            .with_context(|| format!("parsing {}", old.join("state.json").display()))?;
        let field = |name: &str| {
            state
                .pointer(&format!("/active/{name}"))
                .and_then(serde_json::Value::as_str)
        };
        let (Some(directory), Some(version)) = (field("directory"), field("version")) else {
            continue;
        };
        check_version(version)?;
        let source = old.join("versions").join(directory);
        if !source.join(artifact.entry).is_file() {
            tracing::warn!(%engine, source = %source.display(), "old install has no {}", artifact.entry);
            continue;
        }
        let (layout, version, old) = (layout.clone(), version.to_owned(), old.clone());
        tokio::task::spawn_blocking(move || -> Result<()> {
            let target = layout.version_dir(engine, &version);
            if !target.exists() {
                let staging = layout.versions(engine).join(format!(".staging-{version}"));
                let _ = fs::remove_dir_all(&staging);
                link_tree(&source, &staging)
                    .with_context(|| format!("copying {}", source.display()))?;
                fs::rename(&staging, &target)?;
            }
            let home = layout.home(engine);
            if old.join("home").is_dir() && !home.exists() {
                copy_home(&old.join("home"), &home)
                    .with_context(|| format!("copying {}", old.join("home").display()))?;
            }
            switch(&layout, engine, &version)
        })
        .await
        .context("import task")??;
        tracing::info!(%engine, "imported the old install");
        imported.push(engine);
    }
    Ok(imported)
}

/// Engines append to their session files and databases in place, so a
/// hard-linked home would keep writing into the old Forge's files. Copy
/// everything except toolchain caches, whose files are only ever replaced.
fn copy_home(from: &Path, to: &Path) -> io::Result<()> {
    const SHARED_CACHES: [&str; 4] = [".cargo", ".rustup", ".cache", ".npm"];
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let (source, target) = (entry.path(), to.join(entry.file_name()));
        let kind = entry.file_type()?;
        let shared = entry
            .file_name()
            .to_str()
            .is_some_and(|name| SHARED_CACHES.contains(&name));
        if kind.is_dir() && shared {
            link_tree(&source, &target)?;
        } else if kind.is_dir() {
            copy_tree(&source, &target)?;
        } else if kind.is_symlink() {
            #[cfg(unix)]
            std::os::unix::fs::symlink(fs::read_link(&source)?, &target)?;
        } else {
            fs::copy(&source, &target)?;
        }
    }
    Ok(())
}

fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let (source, target) = (entry.path(), to.join(entry.file_name()));
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_tree(&source, &target)?;
        } else if kind.is_symlink() {
            #[cfg(unix)]
            std::os::unix::fs::symlink(fs::read_link(&source)?, &target)?;
        } else {
            fs::copy(&source, &target)?;
        }
    }
    Ok(())
}

fn link_tree(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let (source, target) = (entry.path(), to.join(entry.file_name()));
        let kind = entry.file_type()?;
        if kind.is_dir() {
            link_tree(&source, &target)?;
        } else if kind.is_symlink() {
            #[cfg(unix)]
            std::os::unix::fs::symlink(fs::read_link(&source)?, &target)?;
        } else if fs::hard_link(&source, &target).is_err() {
            fs::copy(&source, &target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Engines;
    use std::collections::HashMap;
    use std::io::Write as _;
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
                let response = match files.get(path) {
                    Some(body) => [
                        format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n", body.len())
                            .into_bytes(),
                        body.clone(),
                    ]
                    .concat(),
                    None => b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n".to_vec(),
                };
                let _ = socket.write_all(&response).await;
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
            seen.push(std::mem::discriminant(&status.install));
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
    async fn installs_from_a_zip() {
        let data = tempfile::tempdir().unwrap();
        let engine = EngineId::Grok;
        let artifact = Artifact {
            archive: Archive::Zip,
            strip: "pkg/",
            ..host_spec(engine).artifact.unwrap()
        };
        let mut zip = zip::ZipWriter::new(io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default().unix_permissions(0o755);
        zip.add_directory("pkg/lib/", options).unwrap();
        zip.start_file("pkg/lib/helper.txt", options).unwrap();
        zip.write_all(b"helper").unwrap();
        zip.start_file(format!("pkg/{}", artifact.entry), options)
            .unwrap();
        zip.write_all(&script("1.0.60")).unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        let hex = sha2::Sha256::digest(&bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let size = bytes.len() as u64;
        let base = serve(HashMap::from([("/grok.zip".to_owned(), bytes)])).await;

        let engines = Engines::new(data.path());
        let resolved = Resolved {
            version: "1.0.60".into(),
            url: format!("{base}/grok.zip"),
            digest: Expected::Sha256 { hex, size },
        };
        engines
            .install_resolved(engine, artifact, resolved)
            .await
            .unwrap();

        assert_eq!(
            engines.status(engine).install,
            InstallState::Installed {
                version: "1.0.60".into(),
                update: None
            }
        );
        let binary = engines.binary(engine).unwrap();
        assert!(fs::read_to_string(&binary).unwrap().contains("1.0.60"));
        assert_eq!(
            fs::read_to_string(binary.parent().unwrap().join("lib/helper.txt")).unwrap(),
            "helper"
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

    #[tokio::test]
    async fn imports_the_old_toolchain_layout() {
        let old = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let grok = old.path().join("grok");
        fs::create_dir_all(grok.join("versions/generation-abc")).unwrap();
        fs::create_dir_all(grok.join("versions/generation-old")).unwrap();
        fs::create_dir_all(grok.join("home/.grok")).unwrap();
        fs::write(grok.join("versions/generation-abc/grok"), script("1.0.50")).unwrap();
        fs::write(grok.join("home/.grok/auth.json"), "{}").unwrap();
        fs::write(
            grok.join("state.json"),
            r#"{"active":{"binary":"grok","directory":"generation-abc","version":"1.0.50"},"format_version":2,
                "previous":[{"binary":"grok","directory":"generation-old","version":"1.0.46"}]}"#,
        )
        .unwrap();

        let engines = Engines::new(data.path());
        assert_eq!(
            engines.import_legacy(old.path()).await.unwrap(),
            [EngineId::Grok]
        );

        assert_eq!(
            engines.status(EngineId::Grok).install,
            InstallState::Installed {
                version: "1.0.50".into(),
                update: None
            }
        );
        assert!(
            fs::read_to_string(engines.binary(EngineId::Grok).unwrap())
                .unwrap()
                .contains("1.0.50")
        );
        assert_eq!(
            fs::read_to_string(engines.home(EngineId::Grok).join(".grok/auth.json")).unwrap(),
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
