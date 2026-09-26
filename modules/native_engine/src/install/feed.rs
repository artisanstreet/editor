//! Vendor release feeds: request locations and strict, bounded parsing.
//!
//! Everything here is pure: callers fetch the bytes through a
//! [`super::transport::ReleaseTransport`] and hand them in, so the parsers are
//! tested with fixture documents and never touch the network. Artifact URLs
//! are constructed or validated against the vendor origin; a feed can never
//! redirect an install to another host.

use std::{collections::BTreeMap, fmt};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{
    Deserialize, Deserializer,
    de::{IgnoredAny, MapAccess, Visitor},
};

use super::{
    authority::decode_sha256,
    catalog::{Feed, VersionFilter},
    version::EngineVersion,
};

const CLAUDE_RELEASES: &str = "https://downloads.claude.ai/claude-code-releases";
const NPM_REGISTRY: &str = "https://registry.npmjs.org";
const CLAUDE_NPM_PACKAGE: &str = "@anthropic-ai/claude-code";
const GROK_RELEASES: &str = "https://x.ai/cli";
const CURSOR_INSTALLER: &str = "https://cursor.com/install";
const CURSOR_RELEASES: &str = "https://downloads.cursor.com/lab";

/// Maximum bytes accepted for a latest-version pointer or dist-tags document.
pub const MAX_POINTER_BYTES: u64 = 64 * 1024;
/// Maximum bytes accepted for one release manifest or version document.
pub const MAX_RELEASE_DOCUMENT_BYTES: u64 = 1024 * 1024;
/// Maximum bytes accepted for a version listing document.
pub const MAX_LISTING_BYTES: u64 = 32 * 1024 * 1024;
/// Maximum versions returned by a listing, newest first.
pub const MAX_LISTED_VERSIONS: usize = 40;

/// Where a document is fetched from and how it must be requested.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FeedRequest {
    pub url: String,
    /// `Accept` header value, when the endpoint needs one.
    pub accept: Option<&'static str>,
    pub bound_bytes: u64,
}

/// The vendor-published digest an artifact must match.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArtifactDigest {
    /// SHA-256 of the downloaded bytes (Claude release manifest).
    Sha256([u8; 32]),
    /// SHA-512 of the downloaded bytes (npm `dist.integrity`).
    Sha512([u8; 64]),
    /// No vendor digest: the first download records its SHA-256 and size,
    /// and every later download of the same version must match them.
    TrustOnFirstDownload,
}

/// One resolved, digest-bearing release artifact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseArtifact {
    pub version: EngineVersion,
    pub url: String,
    pub digest: ArtifactDigest,
    /// Exact artifact size when the vendor publishes it.
    pub size_bytes: Option<u64>,
}

/// A feed document that failed strict validation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FeedError {
    Malformed,
    VersionInvalid,
    VersionMismatch,
    PlatformMissing,
    DigestInvalid,
    UntrustedLocation,
}

impl FeedError {
    /// Returns the stable classification.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Malformed => "feed_malformed",
            Self::VersionInvalid => "feed_version_invalid",
            Self::VersionMismatch => "feed_version_mismatch",
            Self::PlatformMissing => "feed_platform_missing",
            Self::DigestInvalid => "feed_digest_invalid",
            Self::UntrustedLocation => "feed_untrusted_location",
        }
    }
}

/// Returns the request naming the vendor's current version.
#[must_use]
pub fn latest_request(feed: Feed) -> FeedRequest {
    match feed {
        Feed::ClaudeReleases { .. } => FeedRequest {
            url: format!("{CLAUDE_RELEASES}/latest"),
            accept: None,
            bound_bytes: MAX_POINTER_BYTES,
        },
        Feed::Npm { package, .. } => FeedRequest {
            url: format!(
                "{NPM_REGISTRY}/-/package/{}/dist-tags",
                encode_package(package)
            ),
            accept: Some("application/json"),
            bound_bytes: MAX_POINTER_BYTES,
        },
        Feed::GrokReleases { .. } => FeedRequest {
            url: format!("{GROK_RELEASES}/stable"),
            accept: None,
            bound_bytes: MAX_POINTER_BYTES,
        },
        Feed::CursorReleases { .. } => FeedRequest {
            url: CURSOR_INSTALLER.to_owned(),
            accept: None,
            bound_bytes: MAX_POINTER_BYTES,
        },
    }
}

/// Parses the current version from the document fetched for
/// [`latest_request`].
///
/// # Errors
///
/// Returns [`FeedError`] when the document is malformed or names an invalid
/// or off-channel version.
pub fn parse_latest(feed: Feed, bytes: &[u8]) -> Result<EngineVersion, FeedError> {
    match feed {
        Feed::ClaudeReleases { .. } => {
            let text = std::str::from_utf8(bytes).map_err(|_| FeedError::Malformed)?;
            let version = EngineVersion::parse(text.trim()).ok_or(FeedError::VersionInvalid)?;
            if version.has_suffix() {
                return Err(FeedError::VersionInvalid);
            }
            Ok(version)
        }
        Feed::Npm {
            dist_tag, versions, ..
        } => {
            let tags: BTreeMap<String, String> =
                serde_json::from_slice(bytes).map_err(|_| FeedError::Malformed)?;
            let version = tags
                .get(dist_tag)
                .and_then(|value| EngineVersion::parse(value))
                .ok_or(FeedError::VersionInvalid)?;
            if !versions.accepts(&version) {
                return Err(FeedError::VersionInvalid);
            }
            Ok(version)
        }
        Feed::GrokReleases { .. } => {
            let text = std::str::from_utf8(bytes).map_err(|_| FeedError::Malformed)?;
            EngineVersion::parse(text.trim()).ok_or(FeedError::VersionInvalid)
        }
        Feed::CursorReleases { .. } => {
            // The official installer script names its release in the
            // package URL it downloads.
            let text = std::str::from_utf8(bytes).map_err(|_| FeedError::Malformed)?;
            let marker = format!("{CURSOR_RELEASES}/");
            let start = text.find(&marker).ok_or(FeedError::VersionInvalid)? + marker.len();
            let version = text[start..].split('/').next().unwrap_or_default();
            EngineVersion::parse(version).ok_or(FeedError::VersionInvalid)
        }
    }
}

/// Returns the request for the release document of one exact version, or
/// `None` when the vendor publishes none (see [`direct_release`]).
#[must_use]
pub fn release_request(feed: Feed, version: &EngineVersion) -> Option<FeedRequest> {
    Some(match feed {
        Feed::GrokReleases { .. } | Feed::CursorReleases { .. } => return None,
        Feed::ClaudeReleases { .. } => FeedRequest {
            url: format!("{CLAUDE_RELEASES}/{version}/manifest.json"),
            accept: None,
            bound_bytes: MAX_RELEASE_DOCUMENT_BYTES,
        },
        Feed::Npm {
            package,
            platform_suffix,
            ..
        } => FeedRequest {
            url: format!(
                "{NPM_REGISTRY}/{}/{version}{platform_suffix}",
                encode_package(package)
            ),
            accept: Some("application/json"),
            bound_bytes: MAX_RELEASE_DOCUMENT_BYTES,
        },
    })
}

/// Returns the artifact of one exact version for a vendor that publishes no
/// release document: the URL its official installer downloads, verified by
/// trust on first download.
#[must_use]
pub fn direct_release(feed: Feed, version: &EngineVersion) -> Option<ReleaseArtifact> {
    let url = match feed {
        Feed::GrokReleases {
            platform_key,
            binary,
        } => {
            let suffix = if std::path::Path::new(binary)
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
            {
                ".exe"
            } else {
                ""
            };
            format!("{GROK_RELEASES}/grok-{version}-{platform_key}{suffix}")
        }
        Feed::CursorReleases { os, arch } => {
            format!("{CURSOR_RELEASES}/{version}/{os}/{arch}/agent-cli-package.tar.gz")
        }
        Feed::ClaudeReleases { .. } | Feed::Npm { .. } => return None,
    };
    Some(ReleaseArtifact {
        version: version.clone(),
        url,
        digest: ArtifactDigest::TrustOnFirstDownload,
        size_bytes: None,
    })
}

#[derive(Deserialize)]
struct ClaudeManifest {
    version: String,
    platforms: BTreeMap<String, ClaudePlatform>,
}

#[derive(Deserialize)]
struct ClaudePlatform {
    binary: String,
    checksum: String,
    size: u64,
}

#[derive(Deserialize)]
struct NpmVersionDocument {
    version: String,
    dist: NpmDist,
}

#[derive(Deserialize)]
struct NpmDist {
    tarball: String,
    integrity: String,
}

/// Parses the release document fetched for [`release_request`] into the
/// artifact location and its vendor-published digest.
///
/// # Errors
///
/// Returns [`FeedError`] when the document is malformed, names another
/// version, lacks this platform, carries no valid digest, or points outside
/// the vendor origin.
pub fn parse_release(
    feed: Feed,
    version: &EngineVersion,
    bytes: &[u8],
) -> Result<ReleaseArtifact, FeedError> {
    match feed {
        Feed::ClaudeReleases {
            platform_key,
            binary,
        } => {
            let manifest: ClaudeManifest =
                serde_json::from_slice(bytes).map_err(|_| FeedError::Malformed)?;
            if manifest.version != version.as_str() {
                return Err(FeedError::VersionMismatch);
            }
            let platform = manifest
                .platforms
                .get(platform_key)
                .ok_or(FeedError::PlatformMissing)?;
            if platform.binary != binary || platform.size == 0 {
                return Err(FeedError::PlatformMissing);
            }
            Ok(ReleaseArtifact {
                version: version.clone(),
                url: format!("{CLAUDE_RELEASES}/{version}/{platform_key}/{binary}"),
                digest: ArtifactDigest::Sha256(
                    decode_sha256(&platform.checksum).ok_or(FeedError::DigestInvalid)?,
                ),
                size_bytes: Some(platform.size),
            })
        }
        Feed::Npm {
            package,
            platform_suffix,
            ..
        } => {
            let document: NpmVersionDocument =
                serde_json::from_slice(bytes).map_err(|_| FeedError::Malformed)?;
            if document.version != format!("{version}{platform_suffix}") {
                return Err(FeedError::VersionMismatch);
            }
            let origin = format!("{NPM_REGISTRY}/{package}/-/");
            let file = document
                .dist
                .tarball
                .strip_prefix(&origin)
                .ok_or(FeedError::UntrustedLocation)?;
            if file.is_empty()
                || !std::path::Path::new(file)
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("tgz"))
                || !file
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
            {
                return Err(FeedError::UntrustedLocation);
            }
            Ok(ReleaseArtifact {
                version: version.clone(),
                url: document.dist.tarball,
                digest: ArtifactDigest::Sha512(
                    decode_npm_sha512(&document.dist.integrity).ok_or(FeedError::DigestInvalid)?,
                ),
                size_bytes: None,
            })
        }
        Feed::GrokReleases { .. } | Feed::CursorReleases { .. } => {
            direct_release(feed, version).ok_or(FeedError::PlatformMissing)
        }
    }
}

/// Returns whether the vendor publishes a version list.
#[must_use]
pub const fn versions_listed(feed: Feed) -> bool {
    matches!(feed, Feed::ClaudeReleases { .. } | Feed::Npm { .. })
}

/// Returns the request listing the vendor's published versions, or `None`
/// when the vendor publishes no listing.
#[must_use]
pub fn versions_request(feed: Feed) -> Option<FeedRequest> {
    let package = match feed {
        Feed::ClaudeReleases { .. } => CLAUDE_NPM_PACKAGE,
        Feed::Npm { package, .. } => package,
        Feed::GrokReleases { .. } | Feed::CursorReleases { .. } => return None,
    };
    Some(FeedRequest {
        url: format!("{NPM_REGISTRY}/{}", encode_package(package)),
        accept: Some("application/vnd.npm.install-v1+json"),
        bound_bytes: MAX_LISTING_BYTES,
    })
}

#[derive(Deserialize)]
struct NpmPackument {
    versions: VersionKeys,
}

/// The keys of the packument's `versions` object; per-version documents are
/// skipped without being materialized.
struct VersionKeys(Vec<String>);

impl<'de> Deserialize<'de> for VersionKeys {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct KeysVisitor;

        impl<'de> Visitor<'de> for KeysVisitor {
            type Value = VersionKeys;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a versions object")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut keys = Vec::new();
                while let Some(key) = map.next_key::<String>()? {
                    map.next_value::<IgnoredAny>()?;
                    keys.push(key);
                }
                Ok(VersionKeys(keys))
            }
        }

        deserializer.deserialize_map(KeysVisitor)
    }
}

/// Parses the listing fetched for [`versions_request`]: channel versions,
/// newest first, at most [`MAX_LISTED_VERSIONS`]. Below-floor versions are
/// kept so callers can show them as unavailable.
///
/// # Errors
///
/// Returns [`FeedError::Malformed`] when the listing is not a package
/// document.
pub fn parse_versions(feed: Feed, bytes: &[u8]) -> Result<Vec<EngineVersion>, FeedError> {
    let filter = match feed {
        Feed::ClaudeReleases { .. } => VersionFilter::Releases,
        Feed::Npm { versions, .. } => versions,
        Feed::GrokReleases { .. } | Feed::CursorReleases { .. } => return Ok(Vec::new()),
    };
    let packument: NpmPackument =
        serde_json::from_slice(bytes).map_err(|_| FeedError::Malformed)?;
    let mut versions = packument
        .versions
        .0
        .iter()
        .filter_map(|key| EngineVersion::parse(key))
        .filter(|version| filter.accepts(version))
        .collect::<Vec<_>>();
    versions.sort_by(|left, right| right.cmp(left));
    versions.dedup();
    versions.truncate(MAX_LISTED_VERSIONS);
    Ok(versions)
}

fn encode_package(package: &str) -> String {
    package.replace('/', "%2f")
}

fn decode_npm_sha512(integrity: &str) -> Option<[u8; 64]> {
    let encoded = integrity.strip_prefix("sha512-")?;
    if encoded.contains(char::is_whitespace) {
        return None;
    }
    STANDARD.decode(encoded).ok()?.try_into().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine_core::{Distribution, HostPlatform, ManagedEngine};
    use std::fmt::Write as _;

    fn feed(engine: ManagedEngine, platform: HostPlatform) -> Feed {
        match engine.distribution(platform) {
            Distribution::Supported(plan) => plan.feed,
            Distribution::Unsupported(_) => panic!("unsupported"),
        }
    }

    fn version(value: &str) -> EngineVersion {
        EngineVersion::parse(value).unwrap()
    }

    #[test]
    fn claude_latest_and_manifest_resolve_the_platform_binary_and_sha256() {
        let feed = feed(ManagedEngine::Claude, HostPlatform::LinuxX64);
        assert_eq!(
            latest_request(feed).url,
            "https://downloads.claude.ai/claude-code-releases/latest"
        );
        assert_eq!(
            parse_latest(feed, b"2.1.283\n").unwrap().as_str(),
            "2.1.283"
        );
        assert!(parse_latest(feed, b"<html>").is_err());
        let wanted = version("2.1.282");
        assert_eq!(
            release_request(feed, &wanted).unwrap().url,
            "https://downloads.claude.ai/claude-code-releases/2.1.282/manifest.json"
        );
        let manifest = br#"{"version":"2.1.282","platforms":{"linux-x64":{"binary":"claude","checksum":"3afe8535c0cc33f0e24f7b25dab7a1727b8b592196f8496a8bc302ba2161eed3","size":238767288}}}"#;
        let artifact = parse_release(feed, &wanted, manifest).unwrap();
        assert_eq!(
            artifact.url,
            "https://downloads.claude.ai/claude-code-releases/2.1.282/linux-x64/claude"
        );
        assert_eq!(artifact.size_bytes, Some(238_767_288));
        assert!(matches!(artifact.digest, ArtifactDigest::Sha256(digest) if digest[0] == 0x3a));
    }

    #[test]
    fn claude_manifest_rejects_mismatch_missing_platform_and_bad_digest() {
        let feed = feed(ManagedEngine::Claude, HostPlatform::LinuxX64);
        let wanted = version("2.1.282");
        for (document, expected) in [
            (
                r#"{"version":"2.1.281","platforms":{}}"#,
                FeedError::VersionMismatch,
            ),
            (
                r#"{"version":"2.1.282","platforms":{"win32-x64":{"binary":"claude.exe","checksum":"00","size":1}}}"#,
                FeedError::PlatformMissing,
            ),
            (
                r#"{"version":"2.1.282","platforms":{"linux-x64":{"binary":"claude","checksum":"XYZ","size":1}}}"#,
                FeedError::DigestInvalid,
            ),
            ("not json", FeedError::Malformed),
        ] {
            assert_eq!(
                parse_release(feed, &wanted, document.as_bytes()),
                Err(expected)
            );
        }
    }

    #[test]
    fn npm_dist_tags_and_version_documents_resolve_integrity_on_the_registry_only() {
        let feed = feed(ManagedEngine::Codex, HostPlatform::LinuxX64);
        assert_eq!(
            latest_request(feed).url,
            "https://registry.npmjs.org/-/package/@openai%2fcodex/dist-tags"
        );
        let tags =
            br#"{"latest":"0.157.1","alpha":"0.158.0-alpha.2.1","linux-x64":"0.157.1-linux-x64"}"#;
        assert_eq!(parse_latest(feed, tags).unwrap().as_str(), "0.157.1");
        let wanted = version("0.156.0");
        assert_eq!(
            release_request(feed, &wanted).unwrap().url,
            "https://registry.npmjs.org/@openai%2fcodex/0.156.0-linux-x64"
        );
        let document = br#"{"version":"0.156.0-linux-x64","dist":{"tarball":"https://registry.npmjs.org/@openai/codex/-/codex-0.156.0-linux-x64.tgz","integrity":"sha512-/PX399ISB715skgBBOtOX5aqLxmPQhYC7wDasZnAJhRrtzt3qgs6kjSnimN9T8OFKv4LDAp+uJdN896tQYaTrA=="}}"#;
        let artifact = parse_release(feed, &wanted, document).unwrap();
        assert!(matches!(artifact.digest, ArtifactDigest::Sha512(_)));
        assert_eq!(artifact.size_bytes, None);

        let redirected = br#"{"version":"0.156.0-linux-x64","dist":{"tarball":"https://evil.example/codex.tgz","integrity":"sha512-/PX399ISB715skgBBOtOX5aqLxmPQhYC7wDasZnAJhRrtzt3qgs6kjSnimN9T8OFKv4LDAp+uJdN896tQYaTrA=="}}"#;
        assert_eq!(
            parse_release(feed, &wanted, redirected),
            Err(FeedError::UntrustedLocation)
        );
        let sha1 = br#"{"version":"0.156.0-linux-x64","dist":{"tarball":"https://registry.npmjs.org/@openai/codex/-/codex-0.156.0-linux-x64.tgz","integrity":"sha1-AAAA"}}"#;
        assert_eq!(
            parse_release(feed, &wanted, sha1),
            Err(FeedError::DigestInvalid)
        );
    }

    #[test]
    fn opencode2_follows_the_beta_channel_not_the_v1_latest() {
        let feed = feed(ManagedEngine::OpenCode2, HostPlatform::WindowsX64);
        let tags = br#"{"latest":"1.18.18","beta":"0.0.0-beta-19271","dev":"0.0.0-dev-19272"}"#;
        assert_eq!(
            parse_latest(feed, tags).unwrap().as_str(),
            "0.0.0-beta-19271"
        );
        let off_channel = br#"{"beta":"1.18.18"}"#;
        assert_eq!(
            parse_latest(feed, off_channel),
            Err(FeedError::VersionInvalid)
        );
    }

    #[test]
    fn version_listing_is_channel_filtered_newest_first_and_bounded() {
        let feed = feed(ManagedEngine::Codex, HostPlatform::LinuxX64);
        assert_eq!(
            versions_request(feed).unwrap().accept,
            Some("application/vnd.npm.install-v1+json")
        );
        let mut listing = String::from(r#"{"name":"@openai/codex","versions":{"#);
        for minor in 100..160 {
            write!(
                listing,
                r#""0.{minor}.0":{{}},"0.{minor}.0-linux-x64":{{}},"#
            )
            .unwrap();
        }
        listing.push_str(r#""0.158.0-alpha.2":{}}}"#);
        let versions = parse_versions(feed, listing.as_bytes()).unwrap();
        assert_eq!(versions.len(), MAX_LISTED_VERSIONS);
        assert_eq!(versions[0].as_str(), "0.159.0");
        assert!(versions.windows(2).all(|pair| pair[0] > pair[1]));
        assert!(versions.iter().all(|version| !version.has_suffix()));

        let claude = feed_for_claude();
        assert_eq!(
            versions_request(claude).unwrap().url,
            "https://registry.npmjs.org/@anthropic-ai%2fclaude-code"
        );
    }

    fn feed_for_claude() -> Feed {
        feed(ManagedEngine::Claude, HostPlatform::WindowsX64)
    }
}
