use crate::bundle;
use crate::ui::Progress;
use artisan_paths::App;
use base64::Engine as _;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const DEFAULT_FEED: &str =
    "https://github.com/artisanstreet/editor/releases/latest/download/release.json";
pub const NIGHTLY_FEED: &str =
    "https://github.com/artisanstreet/editor/releases/download/nightly/release.json";
const MANIFEST: &str = "release.json";
const RELEASES: &str = "https://github.com/artisanstreet/editor/releases/";

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
    Pinned(semver::Version),
    Dir(PathBuf),
}

fn tag(version: &semver::Version) -> String {
    let mut untagged = version.clone();
    untagged.build = semver::BuildMetadata::EMPTY;
    format!("v{untagged}")
}

fn pinned_manifest(version: &semver::Version) -> String {
    format!("{RELEASES}download/{}/{MANIFEST}", tag(version))
}

fn beside(manifest: &str, name: &str) -> String {
    match name {
        MANIFEST => manifest.to_owned(),
        _ => format!(
            "{}/{name}",
            manifest.rsplit_once('/').map_or(manifest, |(base, _)| base)
        ),
    }
}

fn pins(pin: &semver::Version, version: &str) -> bool {
    semver::Version::parse(version).is_ok_and(|version| {
        (pin.major, pin.minor, pin.patch, &pin.pre)
            == (version.major, version.minor, version.patch, &version.pre)
            && (pin.build.is_empty() || pin.build == version.build)
    })
}

#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    #[error("pass either --from or --version")]
    Both,
    #[error("resolve {value}")]
    Resolve {
        value: String,
        #[source]
        source: std::io::Error,
    },
    #[error("unusable version {version:?}")]
    Version {
        version: String,
        #[source]
        source: semver::Error,
    },
}

impl Source {
    pub fn new(
        from: Option<&str>,
        version: Option<&str>,
        bundled: bool,
    ) -> Result<Self, SourceError> {
        Ok(match (from, version) {
            (Some(_), Some(_)) => return Err(SourceError::Both),
            (Some(value), None)
                if value.starts_with("https://") || value.starts_with("http://") =>
            {
                Self::Url(value.to_owned())
            }
            (Some(value), None) => {
                Self::Dir(
                    std::path::absolute(value).map_err(|source| SourceError::Resolve {
                        value: value.to_owned(),
                        source,
                    })?,
                )
            }
            (None, Some(version)) => {
                Self::Pinned(semver::Version::parse(version).map_err(|source| {
                    SourceError::Version {
                        version: version.to_owned(),
                        source,
                    }
                })?)
            }
            (None, None) if bundled || bundle::FORGE.present() => Self::Bundled,
            (None, None) => Self::Url(feed_for(env!("CARGO_PKG_VERSION")).to_owned()),
        })
    }

    pub fn argument(&self) -> String {
        match self {
            Self::Bundled => "this installer".to_owned(),
            Self::Url(url) => url.clone(),
            Self::Pinned(version) => pinned_manifest(version),
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
            Self::Pinned(version) => Some(feed_for(&version.to_string())),
            Self::Bundled => Some(feed_for(env!("CARGO_PKG_VERSION"))),
            Self::Dir(_) => None,
        }
    }

    fn file(&self, name: &str) -> String {
        match self {
            Self::Bundled => name.to_owned(),
            Self::Dir(dir) => dir.join(name).display().to_string(),
            Self::Url(manifest) => beside(manifest, name),
            Self::Pinned(version) => beside(&pinned_manifest(version), name),
        }
    }

    fn immutable(&self, version: &str) -> Result<Self, ResolveError> {
        match self {
            Self::Url(url) if url.starts_with(RELEASES) => Ok(Self::Pinned(
                semver::Version::parse(version).map_err(|source| ResolveError::Version {
                    version: version.to_owned(),
                    source,
                })?,
            )),
            Self::Bundled | Self::Url(_) | Self::Pinned(_) | Self::Dir(_) => Ok(self.clone()),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Carried {
    pub editor: Option<PathBuf>,
    pub linux_ae: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub connect: Duration,
    pub idle: Duration,
    pub metadata: Duration,
    pub metadata_bytes: u64,
    pub artifact: Duration,
    pub artifact_bytes: u64,
}

pub const LIMITS: Limits = Limits {
    connect: Duration::from_secs(15),
    idle: Duration::from_secs(30),
    metadata: Duration::from_secs(60),
    metadata_bytes: 1024 * 1024,
    artifact: Duration::from_mins(30),
    artifact_bytes: 1024 * 1024 * 1024,
};

#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("start the HTTP client")]
    Client(#[source] reqwest::Error),
    #[error("download {location}")]
    Request {
        location: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("{location} is larger than the {limit} bytes it may be")]
    TooLarge { location: String, limit: u64 },
    #[error("downloading {location} took longer than {after:?}")]
    TimedOut { location: String, after: Duration },
    #[error("save the download from {location} in {}", path.display())]
    Save {
        location: String,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub fn http_client(limits: &Limits) -> Result<reqwest::Client, DownloadError> {
    reqwest::Client::builder()
        .user_agent(concat!("ae/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(limits.connect)
        .read_timeout(limits.idle)
        .build()
        .map_err(DownloadError::Client)
}

async fn download(
    http: &reqwest::Client,
    location: &str,
    limit: u64,
    deadline: Duration,
    mut sink: impl FnMut(&[u8]) -> Result<(), DownloadError>,
) -> Result<(), DownloadError> {
    let request = |source| DownloadError::Request {
        location: location.to_owned(),
        source,
    };
    let too_large = || DownloadError::TooLarge {
        location: location.to_owned(),
        limit,
    };
    let transfer = async {
        let mut response = http
            .get(location)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(request)?;
        if response
            .content_length()
            .is_some_and(|length| length > limit)
        {
            return Err(too_large());
        }
        let mut received: u64 = 0;
        while let Some(chunk) = response.chunk().await.map_err(request)? {
            received = received.saturating_add(chunk.len() as u64);
            if received > limit {
                return Err(too_large());
            }
            sink(&chunk)?;
        }
        Ok(())
    };
    match tokio::time::timeout(deadline, transfer).await {
        Ok(result) => result,
        Err(_elapsed) => Err(DownloadError::TimedOut {
            location: location.to_owned(),
            after: deadline,
        }),
    }
}

pub async fn fetch_metadata(
    http: &reqwest::Client,
    location: &str,
    limits: &Limits,
) -> Result<Vec<u8>, DownloadError> {
    let mut bytes = Vec::new();
    download(
        http,
        location,
        limits.metadata_bytes,
        limits.metadata,
        |chunk| {
            bytes.extend_from_slice(chunk);
            Ok(())
        },
    )
    .await?;
    Ok(bytes)
}

pub async fn fetch_artifact(
    http: &reqwest::Client,
    location: &str,
    limits: &Limits,
    folder: &Path,
    progress: Option<&Progress>,
) -> Result<(tempfile::NamedTempFile, String), DownloadError> {
    use std::io::Write as _;
    let save = |source| DownloadError::Save {
        location: location.to_owned(),
        path: folder.to_owned(),
        source,
    };
    let mut file = tempfile::Builder::new()
        .prefix(".download-")
        .tempfile_in(folder)
        .map_err(save)?;
    let mut hasher = Sha256::new();
    download(
        http,
        location,
        limits.artifact_bytes,
        limits.artifact,
        |chunk| {
            file.write_all(chunk).map_err(save)?;
            hasher.update(chunk);
            if let Some(progress) = progress {
                progress.advance(chunk.len() as u64);
            }
            Ok(())
        },
    )
    .await?;
    Ok((file, hex::encode(hasher.finalize())))
}

pub struct Feed {
    manifest: Manifest,
    source: Source,
    assets: Source,
    http: reqwest::Client,
    limits: Limits,
}

pub enum Release {
    Feed(Feed),
    Local { dir: PathBuf },
    Bundled(Carried),
}

#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("read Artisan's release key")]
    Key(#[source] ReleaseKeyError),
    #[error("{} is neither a release nor a local build", dir.display())]
    NotARelease { dir: PathBuf },
    #[error(transparent)]
    Download(#[from] DownloadError),
    #[error(transparent)]
    Fetch(#[from] FetchError),
    #[error("read release.json from {from}")]
    Manifest {
        from: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("unusable release version {version:?}")]
    UnusableVersion { version: String },
    #[error("unusable release version {version:?}")]
    Version {
        version: String,
        #[source]
        source: semver::Error,
    },
    #[error("verify the release from {from}")]
    Signature {
        from: String,
        #[source]
        source: SignatureError,
    },
    #[error("{from} holds release {found}, not the requested {pin}")]
    WrongPin {
        from: String,
        found: String,
        pin: semver::Version,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error("this installer carries no {name}")]
    Bundled { name: String },
    #[error("read {location}")]
    Read {
        location: String,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Download(#[from] DownloadError),
}

#[derive(Debug, thiserror::Error)]
pub enum ReleaseKeyError {
    #[error("the release public key isn't hex")]
    Hex(#[source] hex::FromHexError),
    #[error("the release key is not 32 bytes but {length}")]
    Length { length: usize },
    #[error("the release key isn't an Ed25519 key")]
    Key(#[source] ed25519_dalek::SignatureError),
}

#[derive(Debug, thiserror::Error)]
pub enum SignatureError {
    #[error("decode release.json.sig")]
    Decode(#[source] base64::DecodeError),
    #[error("read release.json.sig")]
    Read(#[source] ed25519_dalek::SignatureError),
    #[error("release.json is not signed by Artisan's release key")]
    Unsigned(#[source] ed25519_dalek::SignatureError),
}

#[derive(Debug, thiserror::Error)]
pub enum FetchCarrierError {
    #[error("release {version} has no installer for {os} {arch}")]
    NoInstaller {
        version: String,
        os: String,
        arch: &'static str,
    },
    #[error("unusable file name {file:?}")]
    UnusableName { file: String },
    #[error("create {}", path.display())]
    CreateDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Download(#[from] DownloadError),
    #[error("{file} is corrupt or was tampered with (SHA-256 {actual}, expected {expected})")]
    Corrupt {
        file: String,
        actual: String,
        expected: String,
    },
    #[error("write {}", path.display())]
    Persist {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Runnable(#[from] MakeRunnableError),
}

#[derive(Debug, thiserror::Error)]
#[error("make {} runnable", path.display())]
pub struct MakeRunnableError {
    path: PathBuf,
    #[source]
    source: std::io::Error,
}

#[derive(Debug, thiserror::Error)]
pub enum ReleaseVersionError {
    #[error("read {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("unusable version in {}", path.display())]
    Unusable { path: PathBuf },
}

#[derive(Debug, thiserror::Error)]
pub enum UnpackError {
    #[error("create {}", path.display())]
    CreateDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("there is no {}", path.display())]
    Missing { path: PathBuf },
    #[error("copy {} to {}", from.display(), to.display())]
    Copy {
        from: PathBuf,
        to: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Runnable(#[from] MakeRunnableError),
    #[error("a release feed installs through its own installer")]
    Feed,
    #[error("find this ae")]
    CurrentExe(#[source] std::io::Error),
    #[error("write {}", path.display())]
    WriteForge {
        path: PathBuf,
        #[source]
        source: artisan_payload::WritePayloadError,
    },
    #[error("this installer carries no {} for {os}", app.display_name())]
    NotCarried { app: App, os: String },
}

pub async fn resolve(
    source: &Source,
    carried: Carried,
    limits: Limits,
) -> Result<Release, ResolveError> {
    resolve_signed(
        source,
        carried,
        &release_key().map_err(ResolveError::Key)?,
        limits,
    )
    .await
}

async fn resolve_signed(
    source: &Source,
    carried: Carried,
    key: &ed25519_dalek::VerifyingKey,
    limits: Limits,
) -> Result<Release, ResolveError> {
    match source {
        Source::Bundled => return Ok(Release::Bundled(carried)),
        Source::Dir(dir) if !dir.join(MANIFEST).is_file() => {
            if !dir.is_dir() {
                return Err(ResolveError::NotARelease { dir: dir.clone() });
            }
            return Ok(Release::Local { dir: dir.clone() });
        }
        Source::Dir(_) | Source::Url(_) | Source::Pinned(_) => {}
    }
    let http = http_client(&limits)?;
    let raw = fetch(&http, source, MANIFEST, &limits).await?;
    let manifest: Manifest =
        serde_json::from_slice(&raw).map_err(|error| ResolveError::Manifest {
            from: source.argument(),
            source: error,
        })?;
    if !usable_name(&manifest.version) {
        return Err(ResolveError::UnusableVersion {
            version: manifest.version,
        });
    }
    let assets = source.immutable(&manifest.version)?;
    let signature = fetch(&http, &assets, &format!("{MANIFEST}.sig"), &limits).await?;
    verify_signature(&raw, &signature, key).map_err(|error| ResolveError::Signature {
        from: source.argument(),
        source: error,
    })?;
    if let Source::Pinned(pin) = source
        && !pins(pin, &manifest.version)
    {
        return Err(ResolveError::WrongPin {
            from: source.argument(),
            found: manifest.version,
            pin: pin.clone(),
        });
    }
    Ok(Release::Feed(Feed {
        manifest,
        source: source.clone(),
        assets,
        http,
        limits,
    }))
}

async fn fetch(
    http: &reqwest::Client,
    source: &Source,
    name: &str,
    limits: &Limits,
) -> Result<Vec<u8>, FetchError> {
    let location = source.file(name);
    match source {
        Source::Bundled => Err(FetchError::Bundled {
            name: name.to_owned(),
        }),
        Source::Dir(_) => std::fs::read(&location).map_err(|error| FetchError::Read {
            location,
            source: error,
        }),
        Source::Url(_) | Source::Pinned(_) => Ok(fetch_metadata(http, &location, limits).await?),
    }
}

fn release_key() -> Result<ed25519_dalek::VerifyingKey, ReleaseKeyError> {
    let key_hex = if cfg!(debug_assertions) {
        std::env::var("ARTISAN_RELEASE_PUBLIC_KEY_HEX")
            .unwrap_or_else(|_| PUBLIC_KEY_HEX.to_owned())
    } else {
        PUBLIC_KEY_HEX.to_owned()
    };
    let key: [u8; 32] = hex::decode(key_hex.trim())
        .map_err(ReleaseKeyError::Hex)?
        .try_into()
        .map_err(|key: Vec<u8>| ReleaseKeyError::Length { length: key.len() })?;
    ed25519_dalek::VerifyingKey::from_bytes(&key).map_err(ReleaseKeyError::Key)
}

fn verify_signature(
    raw: &[u8],
    signature: &[u8],
    key: &ed25519_dalek::VerifyingKey,
) -> Result<(), SignatureError> {
    let text = String::from_utf8_lossy(signature);
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(text.trim())
        .map_err(SignatureError::Decode)?;
    let signature = ed25519_dalek::Signature::from_slice(&bytes).map_err(SignatureError::Read)?;
    key.verify_strict(raw, &signature)
        .map_err(SignatureError::Unsigned)
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

    pub async fn fetch_carrier(
        &self,
        os: &str,
        folder: &Path,
    ) -> Result<PathBuf, FetchCarrierError> {
        let artifact = self
            .artifact(os)
            .ok_or_else(|| FetchCarrierError::NoInstaller {
                version: self.manifest.version.clone(),
                os: os.to_owned(),
                arch: std::env::consts::ARCH,
            })?;
        if !usable_name(&artifact.file) {
            return Err(FetchCarrierError::UnusableName {
                file: artifact.file.clone(),
            });
        }
        let label = format!("Downloading Artisan {}", self.manifest.version);
        std::fs::create_dir_all(folder).map_err(|source| FetchCarrierError::CreateDir {
            path: folder.to_owned(),
            source,
        })?;
        let location = self.assets.file(&artifact.file);
        let progress = match &self.assets {
            Source::Url(_) | Source::Pinned(_) => match self.announced_length(&location).await {
                Some(length) => Progress::download(&label, length),
                None => Progress::spinner(&label),
            },
            Source::Dir(_) | Source::Bundled => Progress::spinner(&label),
        };
        let downloaded = match &self.assets {
            Source::Url(_) | Source::Pinned(_) => {
                fetch_artifact(&self.http, &location, &self.limits, folder, Some(&progress)).await
            }
            Source::Dir(_) | Source::Bundled => copy_artifact(&location, folder),
        };
        let (file, actual) = match downloaded {
            Ok(downloaded) => downloaded,
            Err(error) => {
                progress.fail(diagnostics::chain(&error));
                return Err(error.into());
            }
        };
        if !actual.eq_ignore_ascii_case(&artifact.sha256) {
            let error = FetchCarrierError::Corrupt {
                file: artifact.file.clone(),
                actual,
                expected: artifact.sha256.clone(),
            };
            progress.fail(&error);
            return Err(error);
        }
        let path = folder.join(&artifact.file);
        file.persist(&path)
            .map_err(|error| FetchCarrierError::Persist {
                path: path.clone(),
                source: error.error,
            })?;
        make_runnable(&path)?;
        progress.done(format!("Downloaded Artisan {}", self.manifest.version));
        Ok(path)
    }

    async fn announced_length(&self, location: &str) -> Option<u64> {
        let head = self.http.head(location).send();
        match tokio::time::timeout(self.limits.metadata, head).await {
            Ok(Ok(response)) => response.content_length(),
            Ok(Err(error)) => {
                tracing::debug!(
                    location,
                    "asking for the download's size failed: {}",
                    diagnostics::chain(&error)
                );
                None
            }
            Err(_elapsed) => {
                tracing::debug!(location, "asking for the download's size timed out");
                None
            }
        }
    }
}

fn copy_artifact(
    location: &str,
    folder: &Path,
) -> Result<(tempfile::NamedTempFile, String), DownloadError> {
    let save = |source| DownloadError::Save {
        location: location.to_owned(),
        path: folder.to_owned(),
        source,
    };
    let mut file = tempfile::Builder::new()
        .prefix(".download-")
        .tempfile_in(folder)
        .map_err(save)?;
    let mut source = std::fs::File::open(location).map_err(save)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        use std::io::{Read as _, Write as _};
        let read = source.read(&mut buffer).map_err(save)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        file.write_all(&buffer[..read]).map_err(save)?;
    }
    Ok((file, hex::encode(hasher.finalize())))
}

#[cfg_attr(
    not(unix),
    expect(
        clippy::missing_const_for_fn,
        clippy::unnecessary_wraps,
        reason = "only Unix marks a file runnable"
    )
)]
fn make_runnable(path: &Path) -> Result<(), MakeRunnableError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).map_err(
            |source| MakeRunnableError {
                path: path.to_owned(),
                source,
            },
        )?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

impl Release {
    pub fn version(&self, os: &str) -> Result<String, ReleaseVersionError> {
        match self {
            Self::Feed(feed) => Ok(feed.manifest.version.clone()),
            Self::Bundled(_) => Ok(env!("CARGO_PKG_VERSION").to_owned()),
            Self::Local { dir } => {
                let path = dir.join(os).join("VERSION");
                let version = match std::fs::read_to_string(&path) {
                    Ok(text) => text.trim().to_owned(),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        "0.0.0-local".to_owned()
                    }
                    Err(source) => return Err(ReleaseVersionError::Read { path, source }),
                };
                if !usable_name(&version) {
                    return Err(ReleaseVersionError::Unusable { path });
                }
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

    pub fn unpack(&self, app: App, os: &str, dest: &Path) -> Result<(), UnpackError> {
        let binary = app.binary_name_on(os);
        let here = os == std::env::consts::OS;
        std::fs::create_dir_all(dest).map_err(|source| UnpackError::CreateDir {
            path: dest.to_owned(),
            source,
        })?;
        let target = dest.join(&binary);
        let copy = |from: &Path| -> Result<(), UnpackError> {
            if !from.is_file() {
                return Err(UnpackError::Missing {
                    path: from.to_owned(),
                });
            }
            std::fs::copy(from, &target).map_err(|source| UnpackError::Copy {
                from: from.to_owned(),
                to: target.clone(),
                source,
            })?;
            Ok(make_runnable(&target)?)
        };
        let not_carried = || UnpackError::NotCarried {
            app,
            os: os.to_owned(),
        };
        match self {
            Self::Feed(_) => Err(UnpackError::Feed),
            Self::Local { dir } => copy(&dir.join(os).join(&binary)),
            Self::Bundled(carried) => match (app, here) {
                (App::Ae, true) => copy(&std::env::current_exe().map_err(UnpackError::CurrentExe)?),
                (App::Ae, false) => copy(carried.linux_ae.as_deref().ok_or_else(not_carried)?),
                (App::Forge, true) => {
                    bundle::FORGE
                        .write_program(&target)
                        .map_err(|source| UnpackError::WriteForge {
                            path: target.clone(),
                            source,
                        })
                }
                (App::Editor, true) => copy(carried.editor.as_deref().ok_or_else(not_carried)?),
                (App::Forge | App::Editor, false) => Err(not_carried()),
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
