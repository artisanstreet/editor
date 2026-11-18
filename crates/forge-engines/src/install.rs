use crate::engines::{
    Archive, Artifact, CLAUDE_RELEASES, Feed, NPM_REGISTRY, compare_versions, host_spec,
};
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

#[derive(Debug, thiserror::Error)]
#[error("unusable version {version:?}")]
pub struct UnusableVersion {
    pub version: String,
}

pub fn check_version(version: &str) -> Result<(), UnusableVersion> {
    let usable = !version.is_empty()
        && version.len() <= 64
        && !version.starts_with('.')
        && version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b".-_+".contains(&byte));
    if usable {
        Ok(())
    } else {
        Err(UnusableVersion {
            version: version.to_owned(),
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SwitchError {
    #[error("write {}", path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("replace {}", path.display())]
    Replace {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

fn write_pointer(path: &Path, version: &str) -> Result<(), SwitchError> {
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, format!("{version}\n")).map_err(|source| SwitchError::Write {
        path: temporary.clone(),
        source,
    })?;
    fs::rename(&temporary, path).map_err(|source| SwitchError::Replace {
        path: path.to_owned(),
        source,
    })
}

pub fn switch(layout: &Layout, engine: EngineId, version: &str) -> Result<(), SwitchError> {
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

#[derive(Debug, thiserror::Error)]
pub enum UsageLockError {
    #[error("engine home {} has no parent", home.display())]
    NoParent { home: PathBuf },
    #[error("create {}", path.display())]
    Create {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("open the engine usage lock {}", path.display())]
    Open {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

pub fn usage_lock(home: &Path) -> Result<fs::File, UsageLockError> {
    let dir = home.parent().ok_or_else(|| UsageLockError::NoParent {
        home: home.to_owned(),
    })?;
    fs::create_dir_all(dir).map_err(|source| UsageLockError::Create {
        path: dir.to_owned(),
        source,
    })?;
    let path = dir.join("usage.lock");
    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|source| UsageLockError::Open { path, source })
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
        if let Err(error) = removed {
            tracing::warn!(%engine, path = %path.display(), %error, "could not remove an old engine version");
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LockError {
    #[error("create {}", path.display())]
    Create {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("open {}", path.display())]
    Open {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("lock {}", path.display())]
    Lock {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("wait for the install lock task")]
    Task(#[source] tokio::task::JoinError),
}

pub async fn lock(layout: &Layout, engine: EngineId) -> Result<fs::File, LockError> {
    let dir = layout.engine_dir(engine);
    tokio::task::spawn_blocking(move || {
        fs::create_dir_all(dir.join("versions")).map_err(|source| LockError::Create {
            path: dir.clone(),
            source,
        })?;
        let path = dir.join("install.lock");
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(|source| LockError::Open {
                path: path.clone(),
                source,
            })?;
        fs2::FileExt::lock_exclusive(&file).map_err(|source| LockError::Lock { path, source })?;
        Ok(file)
    })
    .await
    .map_err(LockError::Task)?
}

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

#[derive(Debug, thiserror::Error)]
pub enum GetError {
    #[error("fetch {url}")]
    Request {
        url: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("read {url}")]
    Body {
        url: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("parse {url}")]
    Json {
        url: String,
        #[source]
        source: serde_json::Error,
    },
}

async fn get(http: &reqwest::Client, url: &str) -> Result<reqwest::Response, GetError> {
    http.get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|source| GetError::Request {
            url: url.to_owned(),
            source,
        })
}

async fn get_text(http: &reqwest::Client, url: &str) -> Result<String, GetError> {
    get(http, url)
        .await?
        .text()
        .await
        .map_err(|source| GetError::Body {
            url: url.to_owned(),
            source,
        })
}

async fn get_json(http: &reqwest::Client, url: &str) -> Result<serde_json::Value, GetError> {
    let text = get_text(http, url).await?;
    parse_json(url, &text)
}

fn parse_json(url: &str, text: &str) -> Result<serde_json::Value, GetError> {
    serde_json::from_str(text).map_err(|source| GetError::Json {
        url: url.to_owned(),
        source,
    })
}

fn npm_name(package: &str) -> String {
    package.replace('/', "%2f")
}

#[derive(Debug, thiserror::Error)]
pub enum LatestVersionError {
    #[error("ask the feed for the latest version")]
    Get(#[source] GetError),
    #[error("{package} has no {tag:?} tag")]
    NoTag {
        package: &'static str,
        tag: &'static str,
    },
    #[error("{url} names no version")]
    NoVersion { url: String },
    #[error("the feed's latest version is unusable")]
    Unusable(#[source] UnusableVersion),
}

pub async fn latest_version(
    http: &reqwest::Client,
    feed: Feed,
) -> Result<String, LatestVersionError> {
    let version = match feed {
        Feed::Claude { .. } => get_text(http, &format!("{CLAUDE_RELEASES}/latest"))
            .await
            .map_err(LatestVersionError::Get)?
            .trim()
            .to_owned(),
        Feed::Npm {
            package, dist_tag, ..
        } => {
            let url = format!("{NPM_REGISTRY}/-/package/{}/dist-tags", npm_name(package));
            let tags = get_json(http, &url)
                .await
                .map_err(LatestVersionError::Get)?;
            tags.get(dist_tag)
                .and_then(serde_json::Value::as_str)
                .ok_or(LatestVersionError::NoTag {
                    package,
                    tag: dist_tag,
                })?
                .to_owned()
        }
        Feed::Release { repository, .. } => {
            let url =
                format!("https://github.com/{repository}/releases/latest/download/manifest.json");
            get_json(http, &url)
                .await
                .map_err(LatestVersionError::Get)?
                .get("version")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| LatestVersionError::NoVersion { url: url.clone() })?
                .to_owned()
        }
    };
    check_version(&version).map_err(LatestVersionError::Unusable)?;
    Ok(version)
}

const CLAUDE_NPM_PACKAGE: &str = "@anthropic-ai/claude-code";
pub const MAX_LISTED_VERSIONS: usize = 40;

pub async fn published_versions(
    http: &reqwest::Client,
    feed: Feed,
) -> Result<Vec<String>, GetError> {
    let (package, channel) = match feed {
        Feed::Claude { .. } => (CLAUDE_NPM_PACKAGE, "latest"),
        Feed::Npm {
            package, dist_tag, ..
        } => (package, dist_tag),
        Feed::Release { repository, .. } => return release_versions(http, repository).await,
    };
    let url = format!("{NPM_REGISTRY}/{}", npm_name(package));
    let text = http
        .get(&url)
        .header(
            reqwest::header::ACCEPT,
            "application/vnd.npm.install-v1+json",
        )
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|source| GetError::Request {
            url: url.clone(),
            source,
        })?
        .text()
        .await
        .map_err(|source| GetError::Body {
            url: url.clone(),
            source,
        })?;
    let packument = parse_json(&url, &text)?;
    Ok(channel_versions(&packument, channel))
}

async fn release_versions(
    http: &reqwest::Client,
    repository: &str,
) -> Result<Vec<String>, GetError> {
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

#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("resolve the version")]
    Unusable(#[source] UnusableVersion),
    #[error("fetch the manifest for {version}")]
    Get {
        version: String,
        #[source]
        source: GetError,
    },
    #[error("the manifest {url} names another version than {version}")]
    OtherVersion { url: String, version: String },
    #[error("{version} has no {platform} build")]
    NoBuild {
        version: String,
        platform: &'static str,
    },
    #[error("the {platform} build of {version} has no {field}")]
    Missing {
        version: String,
        platform: &'static str,
        field: &'static str,
    },
    #[error("{package} {version} points outside the registry: {tarball}")]
    OutsideRegistry {
        package: &'static str,
        version: String,
        tarball: String,
    },
    #[error("decode the npm integrity of {package} {version}")]
    Integrity {
        package: &'static str,
        version: String,
        #[source]
        source: base64::DecodeError,
    },
}

fn manifest_entry<'a>(
    manifest: &'a serde_json::Value,
    url: &str,
    version: &str,
    platform: &'static str,
) -> Result<&'a serde_json::Value, ResolveError> {
    if manifest.get("version").and_then(serde_json::Value::as_str) != Some(version) {
        return Err(ResolveError::OtherVersion {
            url: url.to_owned(),
            version: version.to_owned(),
        });
    }
    manifest
        .pointer(&format!("/platforms/{platform}"))
        .ok_or_else(|| ResolveError::NoBuild {
            version: version.to_owned(),
            platform,
        })
}

fn missing(version: &str, platform: &'static str, field: &'static str) -> ResolveError {
    ResolveError::Missing {
        version: version.to_owned(),
        platform,
        field,
    }
}

async fn get_manifest(
    http: &reqwest::Client,
    url: &str,
    version: &str,
) -> Result<serde_json::Value, ResolveError> {
    get_json(http, url)
        .await
        .map_err(|source| ResolveError::Get {
            version: version.to_owned(),
            source,
        })
}

pub async fn resolve(
    http: &reqwest::Client,
    feed: Feed,
    version: &str,
) -> Result<Resolved, ResolveError> {
    check_version(version).map_err(ResolveError::Unusable)?;
    let (url, digest) = match feed {
        Feed::Claude { platform, binary } => {
            let url = format!("{CLAUDE_RELEASES}/{version}/manifest.json");
            let manifest = get_manifest(http, &url, version).await?;
            let entry = manifest_entry(&manifest, &url, version, platform)?;
            let hex = entry
                .get("checksum")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| missing(version, platform, "checksum"))?;
            let size = entry
                .get("size")
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| missing(version, platform, "size"))?;
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
            let url = format!("{NPM_REGISTRY}/{}/{version}{suffix}", npm_name(package));
            let document = get_manifest(http, &url, version).await?;
            let tarball = document
                .pointer("/dist/tarball")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| missing(version, "npm", "tarball"))?;
            if !tarball.starts_with(&format!("{NPM_REGISTRY}/{package}/-/")) {
                return Err(ResolveError::OutsideRegistry {
                    package,
                    version: version.to_owned(),
                    tarball: tarball.to_owned(),
                });
            }
            let integrity = document
                .pointer("/dist/integrity")
                .and_then(serde_json::Value::as_str)
                .and_then(|value| value.strip_prefix("sha512-"))
                .ok_or_else(|| missing(version, "npm", "sha512 integrity"))?;
            let digest = base64::engine::general_purpose::STANDARD
                .decode(integrity)
                .map_err(|source| ResolveError::Integrity {
                    package,
                    version: version.to_owned(),
                    source,
                })?;
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

async fn resolve_release(
    http: &reqwest::Client,
    repository: &str,
    platform: &'static str,
    version: &str,
) -> Result<(String, Expected), ResolveError> {
    let release =
        format!("https://github.com/{repository}/releases/download/{RELEASE_TAG}{version}");
    let url = format!("{release}/manifest.json");
    let manifest = get_manifest(http, &url, version).await?;
    let entry = manifest_entry(&manifest, &url, version, platform)?;
    let asset = entry
        .get("asset")
        .and_then(serde_json::Value::as_str)
        .filter(|asset| {
            !asset.is_empty()
                && asset
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b".-_".contains(&byte))
        })
        .ok_or_else(|| missing(version, platform, "usable asset name"))?;
    let hex = entry
        .get("sha256")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| missing(version, platform, "sha256"))?;
    let size = entry
        .get("size")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| missing(version, platform, "size"))?;
    Ok((
        format!("{release}/{asset}"),
        Expected::Sha256 {
            hex: hex.to_ascii_lowercase(),
            size,
        },
    ))
}

enum Hasher {
    Sha256(sha2::Sha256),
    Sha512(sha2::Sha512),
}

#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error("install {} {version}", engine.display_name())]
    Unusable {
        engine: EngineId,
        version: String,
        #[source]
        source: UnusableVersion,
    },
    #[error("download {} {version}", engine.display_name())]
    Download {
        engine: EngineId,
        version: String,
        #[source]
        source: DownloadError,
    },
    #[error("verify {} {version} from {url}", engine.display_name())]
    Verify {
        engine: EngineId,
        version: String,
        url: String,
        #[source]
        source: VerifyError,
    },
    #[error("unpack {} {version}", engine.display_name())]
    Unpack {
        engine: EngineId,
        version: String,
        #[source]
        source: UnpackError,
    },
    #[error("remove the old {} {version} at {}", engine.display_name(), path.display())]
    RemoveOld {
        engine: EngineId,
        version: String,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("move {} {version} into {}", engine.display_name(), path.display())]
    Place {
        engine: EngineId,
        version: String,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("make {} {version} current", engine.display_name())]
    Switch {
        engine: EngineId,
        version: String,
        #[source]
        source: SwitchError,
    },
    #[error("wait for the {} {version} unpack task", engine.display_name())]
    Task {
        engine: EngineId,
        version: String,
        #[source]
        source: tokio::task::JoinError,
    },
}

enum PlaceStep {
    Unpack(UnpackError),
    RemoveOld(PathBuf, io::Error),
    Place(PathBuf, io::Error),
    Switch(SwitchError),
}

pub async fn fetch(
    layout: &Layout,
    http: &reqwest::Client,
    engine: EngineId,
    artifact: Artifact,
    resolved: &Resolved,
    report: &(dyn Fn(Event) + Send + Sync),
) -> Result<(), FetchError> {
    let version = &resolved.version;
    check_version(version).map_err(|source| FetchError::Unusable {
        engine,
        version: version.clone(),
        source,
    })?;
    let versions = layout.versions(engine);
    let download = versions.join(format!(".download-{version}"));
    let staging = versions.join(format!(".staging-{version}"));
    report(Event::Start {
        version: version.clone(),
    });
    let result = async {
        let (size, digest) = download_to(http, &resolved.url, &download, &resolved.digest, report)
            .await
            .map_err(|source| FetchError::Download {
                engine,
                version: version.clone(),
                source,
            })?;
        report(Event::Downloaded);
        verify(&resolved.digest, size, &digest).map_err(|source| FetchError::Verify {
            engine,
            version: version.clone(),
            url: resolved.url.clone(),
            source,
        })?;
        let (from, to, target) = (
            download.clone(),
            staging.clone(),
            layout.version_dir(engine, version),
        );
        let switching = layout.clone();
        let switching_version = version.clone();
        let placed = tokio::task::spawn_blocking(move || -> Result<(), PlaceStep> {
            unpack(artifact, &from, &to).map_err(PlaceStep::Unpack)?;
            if target.exists() {
                fs::remove_dir_all(&target)
                    .map_err(|source| PlaceStep::RemoveOld(target.clone(), source))?;
            }
            fs::rename(&to, &target).map_err(|source| PlaceStep::Place(target.clone(), source))?;
            switch(&switching, engine, &switching_version).map_err(PlaceStep::Switch)
        })
        .await
        .map_err(|source| FetchError::Task {
            engine,
            version: version.clone(),
            source,
        })?;
        placed.map_err(|step| {
            let version = version.clone();
            match step {
                PlaceStep::Unpack(source) => FetchError::Unpack {
                    engine,
                    version,
                    source,
                },
                PlaceStep::RemoveOld(path, source) => FetchError::RemoveOld {
                    engine,
                    version,
                    path,
                    source,
                },
                PlaceStep::Place(path, source) => FetchError::Place {
                    engine,
                    version,
                    path,
                    source,
                },
                PlaceStep::Switch(source) => FetchError::Switch {
                    engine,
                    version,
                    source,
                },
            }
        })
    }
    .await;
    if let Err(error) = fs::remove_file(&download)
        && error.kind() != io::ErrorKind::NotFound
    {
        tracing::warn!(%engine, path = %download.display(), %error, "remove a leftover engine download");
    }
    if let Err(error) = fs::remove_dir_all(&staging)
        && error.kind() != io::ErrorKind::NotFound
    {
        tracing::warn!(%engine, path = %staging.display(), %error, "remove a leftover engine staging folder");
    }
    result
}

#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("start the download")]
    Get(#[source] GetError),
    #[error("{url} is {total} bytes, more than we accept")]
    TooLarge { url: String, total: u64 },
    #[error("{url} is larger than we accept")]
    Oversized { url: String },
    #[error("create {}", path.display())]
    Create {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("download {url}")]
    Chunk {
        url: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("write {}", path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("sync {}", path.display())]
    Sync {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

async fn download_to(
    http: &reqwest::Client,
    url: &str,
    path: &Path,
    expected: &Expected,
    report: &(dyn Fn(Event) + Send + Sync),
) -> Result<(u64, Vec<u8>), DownloadError> {
    let mut response = get(http, url).await.map_err(DownloadError::Get)?;
    let total = response.content_length();
    if let Some(total) = total
        && total > MAX_DOWNLOAD_BYTES
    {
        return Err(DownloadError::TooLarge {
            url: url.to_owned(),
            total,
        });
    }
    let write_error = |source| DownloadError::Write {
        path: path.to_owned(),
        source,
    };
    let mut file = tokio::fs::File::create(path)
        .await
        .map_err(|source| DownloadError::Create {
            path: path.to_owned(),
            source,
        })?;
    let mut hasher = match expected {
        Expected::Sha256 { .. } => Hasher::Sha256(sha2::Sha256::new()),
        Expected::Sha512(_) => Hasher::Sha512(sha2::Sha512::new()),
    };
    let (mut received, mut reported) = (0_u64, 0_u64);
    report(Event::Progress { received, total });
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|source| DownloadError::Chunk {
            url: url.to_owned(),
            source,
        })?
    {
        received += chunk.len() as u64;
        if received > MAX_DOWNLOAD_BYTES {
            return Err(DownloadError::Oversized {
                url: url.to_owned(),
            });
        }
        match &mut hasher {
            Hasher::Sha256(hasher) => hasher.update(&chunk),
            Hasher::Sha512(hasher) => hasher.update(&chunk),
        }
        file.write_all(&chunk).await.map_err(write_error)?;
        if received - reported >= PROGRESS_STEP_BYTES {
            reported = received;
            report(Event::Progress { received, total });
        }
    }
    file.flush().await.map_err(write_error)?;
    file.sync_all()
        .await
        .map_err(|source| DownloadError::Sync {
            path: path.to_owned(),
            source,
        })?;
    report(Event::Progress { received, total });
    let digest = match hasher {
        Hasher::Sha256(hasher) => hasher.finalize().to_vec(),
        Hasher::Sha512(hasher) => hasher.finalize().to_vec(),
    };
    Ok((received, digest))
}

#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    #[error("size {size} does not match the published {want}")]
    Size { size: u64, want: u64 },
    #[error("SHA-256 {got} does not match the published {want}")]
    Sha256 { got: String, want: String },
    #[error("SHA-512 does not match the published integrity")]
    Sha512,
}

fn verify(expected: &Expected, size: u64, digest: &[u8]) -> Result<(), VerifyError> {
    match expected {
        Expected::Sha256 { hex, size: want } => {
            if size != *want {
                return Err(VerifyError::Size { size, want: *want });
            }
            let got = hex::encode(digest);
            if got != *hex {
                return Err(VerifyError::Sha256 {
                    got,
                    want: hex.clone(),
                });
            }
        }
        Expected::Sha512(want) => {
            if digest != want.as_slice() {
                return Err(VerifyError::Sha512);
            }
        }
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum UnpackError {
    #[error("clear {}", path.display())]
    Clear {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("create {}", path.display())]
    Create {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("move the download to {}", path.display())]
    Move {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("open {}", path.display())]
    Open {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("read the archive {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("unsafe archive member {}", member.display())]
    Unsafe { member: PathBuf },
    #[error("the archive links {} elsewhere", member.display())]
    Link { member: PathBuf },
    #[error("unpack {}", member.display())]
    Member {
        member: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("the download has no {entry}")]
    NoEntry { entry: &'static str },
    #[error("make {} executable", path.display())]
    Permissions {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "every other tar entry type (devices, fifos, GNU and PAX headers) is skipped"
)]
fn unpack(artifact: Artifact, download: &Path, dest: &Path) -> Result<(), UnpackError> {
    let create = |path: &Path| {
        fs::create_dir_all(path).map_err(|source| UnpackError::Create {
            path: path.to_owned(),
            source,
        })
    };
    if dest.exists() {
        fs::remove_dir_all(dest).map_err(|source| UnpackError::Clear {
            path: dest.to_owned(),
            source,
        })?;
    }
    create(dest)?;
    match artifact.archive {
        Archive::Binary => {
            let to = dest.join(artifact.entry);
            fs::rename(download, &to)
                .or_else(|_| fs::copy(download, &to).map(drop))
                .map_err(|source| UnpackError::Move { path: to, source })?;
        }
        Archive::TarGz => {
            let read = |source| UnpackError::Read {
                path: download.to_owned(),
                source,
            };
            let file = fs::File::open(download).map_err(|source| UnpackError::Open {
                path: download.to_owned(),
                source,
            })?;
            let mut archive =
                tar::Archive::new(flate2::read::GzDecoder::new(io::BufReader::new(file)));
            for entry in archive.entries().map_err(read)? {
                let mut entry = entry.map_err(read)?;
                let path = entry.path().map_err(read)?.into_owned();
                let Some(relative) = inside(&path, artifact.strip)? else {
                    continue;
                };
                let to = dest.join(&relative);
                match entry.header().entry_type() {
                    tar::EntryType::Directory => create(&to)?,
                    tar::EntryType::Regular | tar::EntryType::Continuous => {
                        if let Some(parent) = to.parent() {
                            create(parent)?;
                        }
                        entry.unpack(&to).map_err(|source| UnpackError::Member {
                            member: relative,
                            source,
                        })?;
                    }
                    kind if kind.is_symlink() || kind.is_hard_link() => {
                        return Err(UnpackError::Link { member: path });
                    }
                    _ => {}
                }
            }
        }
    }
    let executable = dest.join(artifact.entry);
    if !executable.is_file() {
        return Err(UnpackError::NoEntry {
            entry: artifact.entry,
        });
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let permissions = |source| UnpackError::Permissions {
            path: executable.clone(),
            source,
        };
        let mode = fs::metadata(&executable)
            .map_err(permissions)?
            .permissions()
            .mode();
        fs::set_permissions(&executable, fs::Permissions::from_mode(mode | 0o755))
            .map_err(permissions)?;
    }
    Ok(())
}

fn inside(path: &Path, strip: &str) -> Result<Option<PathBuf>, UnpackError> {
    let Ok(relative) = path.strip_prefix(strip) else {
        return Ok(None);
    };
    if relative.as_os_str().is_empty() {
        return Ok(None);
    }
    if !relative
        .components()
        .all(|part| matches!(part, Component::Normal(_)))
    {
        return Err(UnpackError::Unsafe {
            member: path.to_owned(),
        });
    }
    Ok(Some(relative.to_path_buf()))
}

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
        assert!(
            matches!(
                error,
                crate::InstallError::Fetch(FetchError::Verify {
                    source: VerifyError::Sha512,
                    ..
                })
            ),
            "{error}"
        );

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
