use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum InstallerError {
    #[error("unsupported platform: {0}")]
    UnsupportedPlatform(String),
    #[cfg(unix)]
    #[error("could not determine the current user's home directory")]
    MissingHome,
    #[error("invalid trust key: {0}")]
    InvalidTrustKey(String),
    #[error(
        "this development installer build has no embedded trust anchor; pass --public-key or ARTISAN_INSTALLER_PUBLIC_KEY"
    )]
    MissingDevelopmentTrustKey,
    #[error(
        "this release installer build is pinned to its embedded release key and refuses --public-key/ARTISAN_INSTALLER_PUBLIC_KEY"
    )]
    ReleaseTrustOverride,
    #[error(
        "this release installer build has no embedded release trust anchor; rebuild with ARTISAN_RELEASE_KEY_ID and ARTISAN_RELEASE_PUBLIC_KEY_HEX"
    )]
    MissingReleaseTrustAnchor,
    #[error("release manifest key id {actual} does not match the pinned release key id {expected}")]
    UntrustedSigningKey { expected: String, actual: String },
    #[error("existing release {version} is not verifiable against the signed manifest: {reason}")]
    UnverifiedRelease { version: String, reason: String },
    #[error("existing release {version} does not match the signed release artifact")]
    TamperedRelease { version: String },
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
    #[error("installer {current} cannot install a release requiring installer {minimum}")]
    InstallerTooOld { current: String, minimum: String },
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
    #[error("staging cleanup could not be completed")]
    StageCleanupIncomplete,
    #[error("installation already exists for release {0}")]
    ExistingRelease(String),
    #[error("permanent ae executable is missing at {0}")]
    MissingCli(PathBuf),
    #[error(
        "installer lifecycle binary is missing at {0}; release archives must contain bin/installer (bin/installer.exe on Windows)"
    )]
    MissingInstaller(PathBuf),
    #[error("installation state is invalid: {0}")]
    InvalidInstallation(String),
    #[cfg(debug_assertions)]
    #[error("development build guard: {0}")]
    DebugBuildGuard(String),
    #[error("permanent ae {command} failed with status {status}")]
    CliFailed { command: String, status: String },
    #[error("could not find current executable: {0}")]
    CurrentExecutable(#[source] std::io::Error),
    #[error("could not resolve temporary directory: {0}")]
    TemporaryDirectory(#[source] std::io::Error),
    #[error("refusing to delete an installer outside the temporary directory: {0}")]
    UnsafeSelfCleanup(PathBuf),
    #[cfg(unix)]
    #[error("path is not valid UTF-8: {0}")]
    NonUtf8Path(PathBuf),
    #[error("could not start self-cleanup helper: {0}")]
    CleanupHelper(#[source] std::io::Error),
    #[error("installation root is busy")]
    InstallationRootBusy,
    #[error("installation root is unsafe")]
    UnsafeInstallationRoot,
    #[error("installation root changed during operation")]
    InstallationRootChanged,
    #[error("installation root has a pending operation")]
    InstallationRootPending,
    #[error("installer pending marker is invalid")]
    InvalidInstallerMarker,
    #[error("installer lock sentinel is invalid")]
    InvalidInstallerLock,
    #[error("installation activation transaction is ambiguous; no files were changed")]
    InstallationActivationTransactionAmbiguous,
    #[error("installer lifecycle helper could not be started")]
    LifecycleHelper,
    #[error("owned installation path is unsafe")]
    UnsafeOwnedPath,
}

pub type Result<T> = std::result::Result<T, InstallerError>;

pub fn io(path: impl Into<PathBuf>) -> impl FnOnce(std::io::Error) -> InstallerError {
    let path = path.into();
    move |source| InstallerError::FileSystem { path, source }
}
