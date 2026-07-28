use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum BootstrapError {
    #[error("unsupported platform: {0}")]
    UnsupportedPlatform(String),
    #[cfg(unix)]
    #[error("could not determine the current user's home directory")]
    MissingHome,
    #[error("invalid trust key: {0}")]
    InvalidTrustKey(String),
    #[error("release manifest request failed: {0}")]
    ManifestRequest(#[source] reqwest::Error),
    #[error("release manifest exceeded the {0}-byte limit")]
    ManifestTooLarge(u64),
    #[error("release manifest is invalid: {0}")]
    InvalidManifest(#[source] serde_json::Error),
    #[error("release manifest signature is invalid")]
    InvalidSignature,
    #[error("release payload is invalid: {0}")]
    InvalidPayload(#[source] serde_json::Error),
    #[error("release contract is invalid: {0}")]
    InvalidRelease(String),
    #[error("release contains no artifact for component {component} on {target}")]
    MissingArtifact { component: String, target: String },
    #[error("bootstrap {current} cannot install a release requiring bootstrap {minimum}")]
    BootstrapTooOld { current: String, minimum: String },
    #[error("artifact request failed for {url}: {source}")]
    ArtifactRequest {
        url: url::Url,
        #[source]
        source: reqwest::Error,
    },
    #[error("artifact {url} exceeded its declared size")]
    ArtifactTooLarge { url: url::Url },
    #[error("artifact size mismatch: expected {expected} bytes, received {actual}")]
    ArtifactSizeMismatch { expected: u64, actual: u64 },
    #[error("artifact checksum mismatch for {0}")]
    ChecksumMismatch(url::Url),
    #[error("archive entry is unsafe: {0}")]
    UnsafeArchiveEntry(String),
    #[error("archive operation failed: {0}")]
    Archive(String),
    #[error("filesystem operation failed at {path}: {source}")]
    FileSystem {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("installation already exists for release {0}")]
    ExistingRelease(String),
    #[error("permanent ae executable is missing at {0}")]
    MissingCli(PathBuf),
    #[error(
        "installer lifecycle binary is missing at {0}; release archives must contain bin/artisan-bootstrap (bin/artisan-bootstrap.exe on Windows)"
    )]
    MissingBootstrap(PathBuf),
    #[error("installation state is invalid: {0}")]
    InvalidInstallation(String),
    #[error("development build guard: {0}")]
    DebugBuildGuard(String),
    #[error("permanent ae {command} failed with status {status}")]
    CliFailed { command: String, status: String },
    #[error("could not find current executable: {0}")]
    CurrentExecutable(#[source] std::io::Error),
    #[error("could not resolve temporary directory: {0}")]
    TemporaryDirectory(#[source] std::io::Error),
    #[error("refusing to delete a bootstrap outside the temporary directory: {0}")]
    UnsafeSelfCleanup(PathBuf),
    #[cfg(unix)]
    #[error("path is not valid UTF-8: {0}")]
    NonUtf8Path(PathBuf),
    #[error("could not start self-cleanup helper: {0}")]
    CleanupHelper(#[source] std::io::Error),
}

pub type Result<T> = std::result::Result<T, BootstrapError>;

pub fn io(path: impl Into<PathBuf>) -> impl FnOnce(std::io::Error) -> BootstrapError {
    let path = path.into();
    move |source| BootstrapError::FileSystem { path, source }
}
