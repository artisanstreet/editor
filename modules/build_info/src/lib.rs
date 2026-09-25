//! Build identity for every Artisan binary.
//!
//! Identity belongs to the installed payload, not to compilation. Whoever
//! stages a payload (the `dev` runner, release packaging) writes
//! [`RESOURCE_PATH`] into the version root, where the payload manifest covers
//! it like any other file. Binaries read it at runtime from their own
//! location, so a new commit never forces the crates that display it to
//! recompile, and a binary running outside an installed payload (a raw
//! `cargo run`) reports itself as [`BuildIdentity::Unstaged`] instead of
//! pretending to be a release.

#![forbid(unsafe_code)]

use std::{
    fmt,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use serde::{Deserialize, Serialize};

/// Payload-relative location of the build identity document.
pub const RESOURCE_PATH: &str = "resources/build-info.json";

/// Current [`BuildInfo`] document format.
pub const FORMAT_VERSION: u8 = 1;

/// Release channel a payload was built for.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    /// Tagged release.
    Stable,
    /// Pre-release candidate.
    Beta,
    /// Built from every green `master`.
    Nightly,
    /// Built locally by the `dev` runner.
    Dev,
}

impl Channel {
    /// Lowercase wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Beta => "beta",
            Self::Nightly => "nightly",
            Self::Dev => "dev",
        }
    }

    /// Human-readable name.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Stable => "Stable",
            Self::Beta => "Beta",
            Self::Nightly => "Nightly",
            Self::Dev => "Dev",
        }
    }
}

/// The identity document stored at [`RESOURCE_PATH`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildInfo {
    /// Document format, currently [`FORMAT_VERSION`].
    pub format_version: u8,
    /// Product version, e.g. `0.4.0` or `0.4.0-dev.1284+g1a2b3c4d5`.
    pub version: String,
    /// Channel the payload was built for.
    pub channel: Channel,
    /// Full commit hash, when the source was a Git checkout.
    pub commit: Option<String>,
    /// Whether the checkout had uncommitted changes to tracked files.
    pub dirty: bool,
    /// Cargo profile, e.g. `dev`, `performance`, or `release`.
    pub profile: String,
    /// Target the binaries were compiled for, e.g. `x86_64-pc-windows-msvc`.
    pub target: String,
    /// RFC 3339 build time; absent when the stager does not record one.
    pub built_at: Option<String>,
}

/// Why a build identity document could not be read.
#[derive(Debug)]
pub enum BuildInfoError {
    /// The document is missing or unreadable.
    Io(std::io::Error),
    /// The document is not valid JSON for this format.
    Invalid(serde_json::Error),
    /// The document uses a format this binary does not understand.
    UnsupportedFormat(u8),
}

impl fmt::Display for BuildInfoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "build identity is unreadable: {error}"),
            Self::Invalid(error) => write!(formatter, "build identity is invalid: {error}"),
            Self::UnsupportedFormat(version) => {
                write!(
                    formatter,
                    "build identity format {version} is not supported"
                )
            }
        }
    }
}

impl std::error::Error for BuildInfoError {}

impl BuildInfo {
    /// Reads the identity document from one version root.
    ///
    /// # Errors
    ///
    /// Returns [`BuildInfoError`] when the document is missing, malformed, or
    /// in an unsupported format.
    pub fn read(version_root: &Path) -> Result<Self, BuildInfoError> {
        let bytes = std::fs::read(version_root.join(RESOURCE_PATH)).map_err(BuildInfoError::Io)?;
        let info: Self = serde_json::from_slice(&bytes).map_err(BuildInfoError::Invalid)?;
        if info.format_version != FORMAT_VERSION {
            return Err(BuildInfoError::UnsupportedFormat(info.format_version));
        }
        Ok(info)
    }

    /// Serializes the document exactly as stagers write it.
    #[must_use]
    pub fn to_json(&self) -> Vec<u8> {
        let mut bytes = serde_json::to_vec_pretty(self).unwrap_or_default();
        bytes.push(b'\n');
        bytes
    }

    /// Abbreviated commit hash, when known.
    #[must_use]
    pub fn short_commit(&self) -> Option<&str> {
        self.commit
            .as_deref()
            .map(|commit| commit.get(..10).unwrap_or(commit))
    }
}

/// Identity of a binary that runs outside an installed payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UnstagedBuild {
    /// Cargo package version compiled into the binary.
    pub package_version: &'static str,
    /// `debug` or `release`, from the compiled assertions setting.
    pub profile: &'static str,
    /// Operating system the binary runs on.
    pub os: &'static str,
    /// CPU architecture the binary runs on.
    pub arch: &'static str,
}

impl UnstagedBuild {
    /// Describes the running binary from compile-time facts alone.
    #[must_use]
    pub const fn this_binary() -> Self {
        Self {
            package_version: env!("CARGO_PKG_VERSION"),
            profile: if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            },
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
        }
    }
}

/// What a running binary knows about its own build.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BuildIdentity {
    /// The binary belongs to an installed payload with a build identity.
    Installed(BuildInfo),
    /// The binary runs outside an installed payload, or its payload carries
    /// no identity (installed before identities existed).
    Unstaged(UnstagedBuild),
}

impl BuildIdentity {
    /// The identity of the running process, resolved once.
    #[must_use]
    pub fn current() -> &'static Self {
        static CURRENT: OnceLock<BuildIdentity> = OnceLock::new();
        CURRENT.get_or_init(|| {
            std::env::current_exe()
                .ok()
                .map_or(Self::Unstaged(UnstagedBuild::this_binary()), |executable| {
                    Self::for_executable(&executable)
                })
        })
    }

    /// Resolves the identity of the payload `executable` belongs to.
    ///
    /// A versioned binary (`<root>/versions/<v>/bin/<name>`) reads its own
    /// version root. The permanent launcher (`<root>/bin/ae`) is a copy of
    /// the active version's `ae`, so it reads the version that
    /// `<root>/installation.json` names as active.
    #[must_use]
    pub fn for_executable(executable: &Path) -> Self {
        version_roots_for(executable)
            .into_iter()
            .find_map(|version_root| BuildInfo::read(&version_root).ok())
            .map_or(
                Self::Unstaged(UnstagedBuild::this_binary()),
                Self::Installed,
            )
    }

    /// The installed identity, when there is one.
    #[must_use]
    pub const fn installed(&self) -> Option<&BuildInfo> {
        match self {
            Self::Installed(info) => Some(info),
            Self::Unstaged(_) => None,
        }
    }

    /// Product version, or the package version for an unstaged binary.
    #[must_use]
    pub fn version(&self) -> &str {
        match self {
            Self::Installed(info) => &info.version,
            Self::Unstaged(build) => build.package_version,
        }
    }

    /// Short marker for window titles. Stable builds carry none; every other
    /// build names its channel and commit so it can never pass for stable.
    #[must_use]
    pub fn title_marker(&self) -> Option<String> {
        match self {
            Self::Installed(info) if info.channel == Channel::Stable => None,
            Self::Installed(info) => Some(match info.short_commit() {
                Some(commit) => format!(
                    "{} {commit}{}",
                    info.channel.label(),
                    if info.dirty { "+" } else { "" }
                ),
                None => info.channel.label().to_owned(),
            }),
            Self::Unstaged(build) => Some(format!("Unstaged {} build", build.profile)),
        }
    }

    /// In-app badge beside the wordmark: the title marker of installed
    /// non-stable builds. Unstaged binaries are marked only in the OS window
    /// title, so test and fixture renders keep the product's own chrome.
    #[must_use]
    pub fn badge(&self) -> Option<String> {
        match self {
            Self::Installed(_) => self.title_marker(),
            Self::Unstaged(_) => None,
        }
    }
}

impl fmt::Display for BuildIdentity {
    /// One line suitable for `--version` output.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Installed(info) => {
                write!(
                    formatter,
                    "{} ({} channel",
                    info.version,
                    info.channel.as_str()
                )?;
                if let Some(commit) = info.short_commit() {
                    write!(formatter, ", commit {commit}")?;
                    if info.dirty {
                        write!(formatter, " with uncommitted changes")?;
                    }
                }
                write!(formatter, ", {} profile, {}", info.profile, info.target)?;
                if let Some(built_at) = &info.built_at {
                    write!(formatter, ", built {built_at}")?;
                }
                write!(formatter, ")")
            }
            Self::Unstaged(build) => write!(
                formatter,
                "{} (unstaged {} build for {}-{}; not part of an installed payload)",
                build.package_version, build.profile, build.os, build.arch
            ),
        }
    }
}

/// The `--version` line for the running binary, formatted once.
#[must_use]
pub fn version_line() -> &'static str {
    static LINE: OnceLock<String> = OnceLock::new();
    LINE.get_or_init(|| BuildIdentity::current().to_string())
}

/// Candidate version roots for `executable`, most specific first.
fn version_roots_for(executable: &Path) -> Vec<PathBuf> {
    let Some(bin) = executable.parent() else {
        return Vec::new();
    };
    if bin.file_name().is_none_or(|name| name != "bin") {
        return Vec::new();
    }
    let Some(parent) = bin.parent() else {
        return Vec::new();
    };
    let mut roots = vec![parent.to_path_buf()];
    if let Some(active) = active_version(parent) {
        roots.push(parent.join("versions").join(active));
    }
    roots
}

/// The active version named by `<root>/installation.json`, if any.
fn active_version(root: &Path) -> Option<String> {
    #[derive(Deserialize)]
    struct Pointer {
        active_version: Option<String>,
    }
    let bytes = std::fs::read(root.join("installation.json")).ok()?;
    let version = serde_json::from_slice::<Pointer>(&bytes)
        .ok()?
        .active_version?;
    let mut components = Path::new(&version).components();
    let single_component = matches!(components.next(), Some(std::path::Component::Normal(_)))
        && components.next().is_none();
    single_component.then_some(version)
}
