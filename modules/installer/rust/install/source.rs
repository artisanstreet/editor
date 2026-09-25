//! Where a release comes from, and how its payload reaches the stage.
//!
//! Every source ends in the same verified [`ReleaseRecord`] and a payload
//! that materializes into an empty stage directory; everything after that
//! (version checks, activation, retirement, integrations) is shared, so a
//! locally built release is installed exactly the way a published one is.

use std::{
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};
use url::Url;

use crate::{
    archive,
    error::{InstallerError, Result, io},
    manifest::{
        Artifact, ReleaseRecord, TREE_MANIFEST_NAME, TREE_SIGNATURE_NAME, TrustKey, decode,
        decode_tree, fetch,
    },
    platform::Platform,
};

use super::authority::{copy_owned_file, create_owned_file, hash_file, remove_owned_file};

const ABSOLUTE_ARTIFACT_LIMIT: u64 = 2 * 1024 * 1024 * 1024;
const MAX_MANIFEST_FILE_BYTES: u64 = 1024 * 1024;
const NATIVE_PAYLOAD_LABEL: &str = "native payload";

/// File name of a signed release manifest inside a release directory.
pub const RELEASE_MANIFEST_NAME: &str = "release-manifest.json";

/// File name of a release manifest's detached signature.
pub const RELEASE_SIGNATURE_NAME: &str = "release-manifest.sig";

/// Where a release is installed from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReleaseSource {
    /// A signed release manifest served over HTTPS (plain HTTP only from
    /// loopback); artifacts are fetched relative to it.
    Remote {
        /// Signed release manifest.
        manifest_url: Url,
        /// Its detached signature.
        signature_url: Url,
    },
    /// A release directory on disk: exactly what `release-tool` produces, a
    /// signed release manifest beside the artifacts it names.
    Directory {
        /// Directory holding the manifest, signature, and artifacts.
        path: PathBuf,
    },
    /// An unpacked payload on disk (`bin/`, `resources/`) described by a
    /// signed tree manifest, as the `dev` runner produces it.
    Tree {
        /// Payload root holding the tree manifest and signature.
        path: PathBuf,
    },
}

impl ReleaseSource {
    /// A remote release whose signature sits beside its manifest (`.json`
    /// replaced by `.sig`).
    ///
    /// # Errors
    ///
    /// Returns [`InstallerError::InvalidRelease`] when the signature URL
    /// cannot be derived.
    pub fn remote(manifest_url: Url) -> Result<Self> {
        let signature_url = Url::parse(&manifest_url.as_str().replace(".json", ".sig"))
            .map_err(|error| InstallerError::InvalidRelease(error.to_string()))?;
        Ok(Self::Remote {
            manifest_url,
            signature_url,
        })
    }

    /// Interprets a `--from` location: an `http(s)` manifest URL, a release
    /// directory, or an unpacked tree.
    ///
    /// # Errors
    ///
    /// Returns [`InstallerError::InvalidRelease`] when the URL is malformed or
    /// the directory holds neither a release nor a tree manifest.
    pub fn from_location(location: &str) -> Result<Self> {
        if location.starts_with("https://") || location.starts_with("http://") {
            let url = Url::parse(location)
                .map_err(|error| InstallerError::InvalidRelease(error.to_string()))?;
            return Self::remote(url);
        }
        let path = std::path::absolute(location).map_err(io(location))?;
        if path.join(TREE_MANIFEST_NAME).is_file() {
            Ok(Self::Tree { path })
        } else if path.join(RELEASE_MANIFEST_NAME).is_file() {
            Ok(Self::Directory { path })
        } else {
            Err(InstallerError::InvalidRelease(format!(
                "{} holds neither {RELEASE_MANIFEST_NAME} nor {TREE_MANIFEST_NAME}",
                path.display()
            )))
        }
    }
}

/// A release whose manifest verified, ready to materialize.
pub(super) struct Acquired {
    pub(super) record: ReleaseRecord,
    pub(super) payload: Payload,
}

/// The verified payload of an acquired release.
pub(super) enum Payload {
    Download {
        client: reqwest::Client,
        url: Url,
        artifact: Artifact,
    },
    ArchiveFile {
        path: PathBuf,
        artifact: Artifact,
    },
    Tree {
        root: PathBuf,
        files: BTreeMap<String, String>,
    },
}

/// Fetches and verifies the release manifest `source` names.
pub(super) async fn acquire(
    source: &ReleaseSource,
    trust: &TrustKey,
    platform: &Platform,
) -> Result<Acquired> {
    match source {
        ReleaseSource::Remote {
            manifest_url,
            signature_url,
        } => {
            // Plain HTTP is permitted only from this machine's own loopback,
            // which cannot be intercepted off-host; every remote manifest
            // still requires TLS, and the signature check applies to both.
            let loopback = manifest_url.host().is_some_and(|host| match host {
                url::Host::Ipv4(address) => address.is_loopback(),
                url::Host::Ipv6(address) => address.is_loopback(),
                url::Host::Domain(domain) => domain == "localhost",
            });
            let client = reqwest::Client::builder()
                .https_only(!loopback)
                .redirect(reqwest::redirect::Policy::limited(5))
                .build()
                .map_err(InstallerError::ManifestRequest)?;
            let base = manifest_url
                .join("./")
                .map_err(|error| InstallerError::InvalidTrustKey(error.to_string()))?;
            let manifest =
                fetch(&client, manifest_url.clone(), signature_url.clone(), trust).await?;
            let artifact = native_artifact(&manifest.artifacts, platform)?.clone();
            let url = base
                .join(&artifact.file_name)
                .map_err(|error| InstallerError::InvalidTrustKey(error.to_string()))?;
            Ok(Acquired {
                record: manifest.record(&artifact),
                payload: Payload::Download {
                    client,
                    url,
                    artifact,
                },
            })
        }
        ReleaseSource::Directory { path } => {
            let bytes = read_bounded(&path.join(RELEASE_MANIFEST_NAME))?;
            let signature = read_bounded(&path.join(RELEASE_SIGNATURE_NAME))?;
            let manifest = decode(&bytes, &signature, trust)?;
            let artifact = native_artifact(&manifest.artifacts, platform)?.clone();
            if Path::new(&artifact.file_name).file_name()
                != Some(std::ffi::OsStr::new(&artifact.file_name))
            {
                return Err(InstallerError::InvalidRelease(format!(
                    "artifact file name {} is not a plain file name",
                    artifact.file_name
                )));
            }
            Ok(Acquired {
                record: manifest.record(&artifact),
                payload: Payload::ArchiveFile {
                    path: path.join(&artifact.file_name),
                    artifact,
                },
            })
        }
        ReleaseSource::Tree { path } => {
            let bytes = read_bounded(&path.join(TREE_MANIFEST_NAME))?;
            let signature = read_bounded(&path.join(TREE_SIGNATURE_NAME))?;
            let manifest = decode_tree(&bytes, &signature, trust)?;
            if manifest.platform != platform.os || manifest.architecture != platform.arch {
                return Err(InstallerError::MissingArtifact {
                    component: NATIVE_PAYLOAD_LABEL.to_owned(),
                    target: platform.target(),
                });
            }
            if let Some(relative) = manifest
                .files
                .keys()
                .find(|relative| !crate::payload::is_declarable(relative))
            {
                return Err(InstallerError::UnsafeArchiveEntry(relative.clone()));
            }
            let record = manifest.record(hex::encode(Sha256::digest(&bytes)));
            Ok(Acquired {
                record,
                payload: Payload::Tree {
                    root: path.clone(),
                    files: manifest.files,
                },
            })
        }
    }
}

impl Payload {
    /// The signed archive this payload extracts, if it is an archive.
    pub(super) const fn artifact(&self) -> Option<&Artifact> {
        match self {
            Self::Download { artifact, .. } | Self::ArchiveFile { artifact, .. } => Some(artifact),
            Self::Tree { .. } => None,
        }
    }

    /// Materializes the verified payload into the empty `stage`. A tree may
    /// reuse identical files of the active version at `reuse` by hardlink;
    /// everything else is copied, so build outputs are never linked into an
    /// installation (a running installed binary must not pin a build output).
    pub(super) async fn materialize(&self, stage: &Path, reuse: Option<&Path>) -> Result<()> {
        match self {
            Self::Download {
                client,
                url,
                artifact,
            } => install_artifact(client, artifact, url.clone(), stage).await,
            Self::ArchiveFile { path, artifact } => install_archive_file(path, artifact, stage),
            Self::Tree { root, files } => install_tree(root, files, stage, reuse),
        }
    }
}

fn read_bounded(path: &Path) -> Result<Vec<u8>> {
    let metadata = std::fs::metadata(path).map_err(io(path))?;
    if metadata.len() > MAX_MANIFEST_FILE_BYTES {
        return Err(InstallerError::ManifestTooLarge(MAX_MANIFEST_FILE_BYTES));
    }
    std::fs::read(path).map_err(io(path))
}

fn native_artifact<'a>(artifacts: &'a [Artifact], platform: &Platform) -> Result<&'a Artifact> {
    artifacts
        .iter()
        .find(|artifact| {
            artifact.platform == platform.os
                && artifact.architecture == platform.arch
                && (platform.os != "linux" || artifact.libc.as_deref() == Some(platform_libc()))
        })
        .ok_or_else(|| InstallerError::MissingArtifact {
            component: NATIVE_PAYLOAD_LABEL.to_owned(),
            target: platform.target(),
        })
}

pub(crate) fn platform_libc() -> &'static str {
    if cfg!(target_env = "musl") {
        "musl"
    } else {
        "glibc"
    }
}

async fn install_artifact(
    client: &reqwest::Client,
    artifact: &Artifact,
    artifact_url: Url,
    stage: &Path,
) -> Result<()> {
    if artifact.size == 0 || artifact.size > ABSOLUTE_ARTIFACT_LIMIT {
        return Err(InstallerError::ArtifactTooLarge {
            url: artifact_url.clone(),
        });
    }
    let response = client
        .get(artifact_url.clone())
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|source| InstallerError::ArtifactRequest {
            url: artifact_url.clone(),
            source,
        })?;
    if response
        .content_length()
        .is_some_and(|size| size > artifact.size)
    {
        return Err(InstallerError::ArtifactTooLarge {
            url: artifact_url.clone(),
        });
    }
    let download = stage.join(format!(".{}.download", artifact.id));
    let mut file = create_owned_file(&download)?;
    let mut response = response;
    let mut downloaded = 0_u64;
    let mut hasher = Sha256::new();
    while let Some(chunk) =
        response
            .chunk()
            .await
            .map_err(|source| InstallerError::ArtifactRequest {
                url: artifact_url.clone(),
                source,
            })?
    {
        downloaded = downloaded.saturating_add(chunk.len() as u64);
        if downloaded > artifact.size {
            return Err(InstallerError::ArtifactTooLarge {
                url: artifact_url.clone(),
            });
        }
        hasher.update(&chunk);
        file.write_all(&chunk).map_err(io(&download))?;
    }
    if downloaded != artifact.size {
        return Err(InstallerError::ArtifactSizeMismatch {
            expected: artifact.size,
            actual: downloaded,
        });
    }
    file.sync_all().map_err(io(&download))?;
    let digest = hex::encode(hasher.finalize());
    if !digest.eq_ignore_ascii_case(&artifact.sha256) {
        return Err(InstallerError::ChecksumMismatch(artifact_url));
    }
    archive::extract(&download, artifact.format, stage, &artifact.archive_entries)?;
    remove_owned_file(&download)?;
    Ok(())
}

/// Copies a local release archive into the stage, then verifies and extracts
/// the copy, so the source can change afterwards without affecting what was
/// verified.
fn install_archive_file(source: &Path, artifact: &Artifact, stage: &Path) -> Result<()> {
    let size = std::fs::metadata(source).map_err(io(source))?.len();
    if size == 0 || size > ABSOLUTE_ARTIFACT_LIMIT {
        return Err(InstallerError::Archive(format!(
            "release archive {} has an invalid size",
            source.display()
        )));
    }
    if size != artifact.size {
        return Err(InstallerError::ArtifactSizeMismatch {
            expected: artifact.size,
            actual: size,
        });
    }
    let copy = stage.join(format!(".{}.download", artifact.id));
    copy_owned_file(source, &copy)?;
    if !hash_file(&copy)?.eq_ignore_ascii_case(&artifact.sha256) {
        return Err(InstallerError::Archive(format!(
            "release archive {} does not match its signed checksum",
            source.display()
        )));
    }
    archive::extract(&copy, artifact.format, stage, &artifact.archive_entries)?;
    remove_owned_file(&copy)
}

/// Materializes exactly the declared files of an unpacked tree, verifying
/// each staged file against its signed digest.
fn install_tree(
    source: &Path,
    files: &BTreeMap<String, String>,
    stage: &Path,
    reuse: Option<&Path>,
) -> Result<()> {
    let reusable = reuse.and_then(|root| {
        crate::payload::recorded_files(root)
            .ok()
            .map(|files| (root, files))
    });
    for (relative, expected) in files {
        let destination = stage.join(relative);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).map_err(io(parent))?;
        }
        let linked = reusable.as_ref().is_some_and(|(root, recorded)| {
            recorded
                .get(relative)
                .is_some_and(|digest| digest.eq_ignore_ascii_case(expected))
                && std::fs::hard_link(root.join(relative), &destination).is_ok()
        });
        if !linked {
            std::fs::copy(source.join(relative), &destination).map_err(io(&destination))?;
        }
        if !hash_file(&destination)?.eq_ignore_ascii_case(expected) {
            return Err(InstallerError::Archive(format!(
                "staged {relative} does not match its signed digest"
            )));
        }
    }
    Ok(())
}
