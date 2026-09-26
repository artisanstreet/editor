//! The managed engine catalog: which engines the Forge installs, where each
//! vendor publishes releases and digests, and how an artifact is laid out.
//!
//! The catalog holds no versions. Versions are resolved from the vendor feed
//! at install time; the catalog only fixes feed locations, compatibility
//! floors, platform keys, and artifact layouts. A platform whose vendor
//! publishes no digest is [`Distribution::Unsupported`].

use std::fmt;

use super::version::EngineVersion;

/// An engine CLI the Forge installs and launches itself.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub enum ManagedEngine {
    Claude,
    Codex,
    Cursor,
    Grok,
    OpenCode2,
}

impl ManagedEngine {
    /// Every managed engine in stable display order.
    pub const ALL: [Self; 5] = [
        Self::Claude,
        Self::Codex,
        Self::Cursor,
        Self::Grok,
        Self::OpenCode2,
    ];

    /// Returns the stable engine identifier used in paths and protocol.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Cursor => "cursor",
            Self::Grok => "grok",
            Self::OpenCode2 => "opencode2",
        }
    }

    /// Parses a stable engine identifier.
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|engine| engine.id() == id)
    }

    /// Returns the human-readable engine name.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
            Self::Cursor => "Cursor Agent",
            Self::Grok => "Grok Build",
            Self::OpenCode2 => "OpenCode2",
        }
    }

    /// Returns the oldest version Artisan installs, selects, or runs.
    #[must_use]
    pub const fn floor(self) -> &'static str {
        match self {
            Self::Claude => crate::claude_authority::CLAUDE_MINIMUM_CLI_VERSION,
            Self::Codex => crate::codex::version::CODEX_MINIMUM_CLI_VERSION,
            Self::OpenCode2 => "0.0.0-beta-17778",
            Self::Cursor | Self::Grok => "0.0.0",
        }
    }

    /// Returns whether `version` is at or above this engine's floor.
    #[must_use]
    pub fn meets_floor(self, version: &EngineVersion) -> bool {
        EngineVersion::parse(self.floor()).is_some_and(|floor| *version >= floor)
    }

    /// Returns the single documented developer override variable.
    ///
    /// The override is honoured only as an absolute path and is reported as
    /// an override in status; it never enables `PATH` discovery.
    #[must_use]
    pub const fn override_env(self) -> &'static str {
        match self {
            Self::Claude => "ARTISAN_CLAUDE_EXECUTABLE",
            Self::Codex => "ARTISAN_CODEX_EXECUTABLE",
            Self::Cursor => "ARTISAN_CURSOR_EXECUTABLE",
            Self::Grok => "ARTISAN_GROK_EXECUTABLE",
            Self::OpenCode2 => "ARTISAN_OPENCODE2_EXECUTABLE",
        }
    }

    /// Returns how this engine is distributed on `platform`.
    #[must_use]
    pub const fn distribution(self, platform: HostPlatform) -> Distribution {
        match self {
            Self::Claude => claude_distribution(platform),
            Self::Codex => codex_distribution(platform),
            Self::OpenCode2 => opencode2_distribution(platform),
            Self::Grok | Self::Cursor => {
                Distribution::Unsupported(UnsupportedReason::NoVendorDigest)
            }
        }
    }
}

impl fmt::Display for ManagedEngine {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.id())
    }
}

/// The operating system and architecture an engine is installed for.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum HostPlatform {
    LinuxX64,
    LinuxX64Musl,
    LinuxArm64,
    LinuxArm64Musl,
    WindowsX64,
    WindowsArm64,
    MacArm64,
    MacX64,
    Other,
}

impl HostPlatform {
    /// Every concrete platform, for catalog validation.
    pub const ALL: [Self; 8] = [
        Self::LinuxX64,
        Self::LinuxX64Musl,
        Self::LinuxArm64,
        Self::LinuxArm64Musl,
        Self::WindowsX64,
        Self::WindowsArm64,
        Self::MacArm64,
        Self::MacX64,
    ];

    /// Detects the platform of this process's host.
    ///
    /// On Linux the C library is detected from the dynamic loader present on
    /// the host, not from this binary's own target, because engines are
    /// separate executables.
    #[must_use]
    pub fn current() -> Self {
        let musl = cfg!(target_os = "linux")
            && ["/lib/ld-musl-x86_64.so.1", "/lib/ld-musl-aarch64.so.1"]
                .iter()
                .any(|loader| std::path::Path::new(loader).exists());
        match (std::env::consts::OS, std::env::consts::ARCH, musl) {
            ("linux", "x86_64", false) => Self::LinuxX64,
            ("linux", "x86_64", true) => Self::LinuxX64Musl,
            ("linux", "aarch64", false) => Self::LinuxArm64,
            ("linux", "aarch64", true) => Self::LinuxArm64Musl,
            ("windows", "x86_64", _) => Self::WindowsX64,
            ("windows", "aarch64", _) => Self::WindowsArm64,
            ("macos", "aarch64", _) => Self::MacArm64,
            ("macos", "x86_64", _) => Self::MacX64,
            _ => Self::Other,
        }
    }

    /// Returns whether executables on this platform use the `.exe` suffix.
    #[must_use]
    pub const fn is_windows(self) -> bool {
        matches!(self, Self::WindowsX64 | Self::WindowsArm64)
    }

    /// Returns a stable platform label for status and diagnostics.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::LinuxX64 => "linux-x64",
            Self::LinuxX64Musl => "linux-x64-musl",
            Self::LinuxArm64 => "linux-arm64",
            Self::LinuxArm64Musl => "linux-arm64-musl",
            Self::WindowsX64 => "win32-x64",
            Self::WindowsArm64 => "win32-arm64",
            Self::MacArm64 => "darwin-arm64",
            Self::MacX64 => "darwin-x64",
            Self::Other => "unknown",
        }
    }
}

/// Why an engine cannot be managed on a platform.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnsupportedReason {
    /// The vendor publishes no digest for its binaries, so they cannot be
    /// verified before installation.
    NoVendorDigest,
    /// The vendor publishes no build for this platform.
    NoVendorBuild,
}

impl UnsupportedReason {
    /// Returns the stable classification.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::NoVendorDigest => "no_vendor_digest",
            Self::NoVendorBuild => "no_vendor_build",
        }
    }

    /// Returns a user-facing explanation.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::NoVendorDigest => {
                "the vendor publishes no checksum for its binaries, so Artisan cannot verify them"
            }
            Self::NoVendorBuild => "the vendor publishes no build for this platform",
        }
    }
}

/// How an engine is distributed on one platform.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Distribution {
    Supported(ArtifactPlan),
    Unsupported(UnsupportedReason),
}

/// The feed, integrity source, and layout of one engine on one platform.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ArtifactPlan {
    pub feed: Feed,
    pub layout: Layout,
    /// Maximum accepted download size in bytes.
    pub download_bound_bytes: u64,
    /// Maximum total expanded archive size in bytes.
    pub expanded_bound_bytes: u64,
}

/// Where versions, artifact locations, and digests are published.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Feed {
    /// Anthropic's native release bucket used by `claude.ai/install.sh`:
    /// `<base>/latest`, `<base>/<v>/manifest.json` (SHA-256 + size per
    /// platform), `<base>/<v>/<platform>/<binary>`. Versions are listed from
    /// the official npm package that shares the release numbers.
    ClaudeReleases {
        platform_key: &'static str,
        binary: &'static str,
    },
    /// The npm registry: the dist-tag names the current version, the
    /// version document carries the tarball URL and SHA-512 integrity.
    Npm {
        package: &'static str,
        dist_tag: &'static str,
        /// Suffix appended to the release version to name this platform's
        /// build (`@openai/codex@0.156.0-linux-x64`).
        platform_suffix: &'static str,
        versions: VersionFilter,
    },
}

/// Which published versions belong to the managed channel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VersionFilter {
    /// Only exact `X.Y.Z` releases.
    Releases,
    /// Only `X.Y.Z-<prefix><digits>` builds.
    Numbered(&'static str),
}

impl VersionFilter {
    /// Returns whether `version` belongs to the channel.
    #[must_use]
    pub fn accepts(self, version: &EngineVersion) -> bool {
        match self {
            Self::Releases => !version.has_suffix(),
            Self::Numbered(prefix) => version.suffix().is_some_and(|suffix| {
                suffix.strip_prefix(prefix).is_some_and(|number| {
                    !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
                })
            }),
        }
    }
}

/// How the verified download becomes a generation directory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Layout {
    /// The download is the executable itself.
    SingleBinary { binary: &'static str },
    /// One exact member of a gzip tar archive is the executable.
    TarMember {
        member: &'static str,
        binary: &'static str,
    },
    /// Every regular file below `strip` is extracted; `entry` is the
    /// executable and `tool_dirs` are prepended to the engine `PATH`.
    TarTree {
        strip: &'static str,
        entry: &'static str,
        tool_dirs: &'static [&'static str],
    },
}

impl Layout {
    /// Returns the executable path relative to the generation directory.
    #[must_use]
    pub const fn entry(self) -> &'static str {
        match self {
            Self::SingleBinary { binary } | Self::TarMember { binary, .. } => binary,
            Self::TarTree { entry, .. } => entry,
        }
    }

    /// Returns directories (relative to the generation) the engine expects
    /// on its `PATH`.
    #[must_use]
    pub const fn tool_dirs(self) -> &'static [&'static str] {
        match self {
            Self::SingleBinary { .. } | Self::TarMember { .. } => &[],
            Self::TarTree { tool_dirs, .. } => tool_dirs,
        }
    }
}

const MIB: u64 = 1024 * 1024;

const fn claude_distribution(platform: HostPlatform) -> Distribution {
    let (platform_key, binary) = match platform {
        HostPlatform::LinuxX64 => ("linux-x64", "claude"),
        HostPlatform::LinuxX64Musl => ("linux-x64-musl", "claude"),
        HostPlatform::LinuxArm64 => ("linux-arm64", "claude"),
        HostPlatform::LinuxArm64Musl => ("linux-arm64-musl", "claude"),
        HostPlatform::WindowsX64 => ("win32-x64", "claude.exe"),
        HostPlatform::WindowsArm64 => ("win32-arm64", "claude.exe"),
        HostPlatform::MacArm64 => ("darwin-arm64", "claude"),
        HostPlatform::MacX64 => ("darwin-x64", "claude"),
        HostPlatform::Other => {
            return Distribution::Unsupported(UnsupportedReason::NoVendorBuild);
        }
    };
    Distribution::Supported(ArtifactPlan {
        feed: Feed::ClaudeReleases {
            platform_key,
            binary,
        },
        layout: Layout::SingleBinary { binary },
        download_bound_bytes: 512 * MIB,
        expanded_bound_bytes: 512 * MIB,
    })
}

const fn codex_distribution(platform: HostPlatform) -> Distribution {
    let (platform_suffix, entry, tool_dirs): (_, _, &'static [&'static str]) = match platform {
        HostPlatform::LinuxX64 | HostPlatform::LinuxX64Musl => (
            "-linux-x64",
            "vendor/x86_64-unknown-linux-musl/bin/codex",
            &["vendor/x86_64-unknown-linux-musl/codex-path"],
        ),
        HostPlatform::LinuxArm64 | HostPlatform::LinuxArm64Musl => (
            "-linux-arm64",
            "vendor/aarch64-unknown-linux-musl/bin/codex",
            &["vendor/aarch64-unknown-linux-musl/codex-path"],
        ),
        HostPlatform::WindowsX64 => (
            "-win32-x64",
            "vendor/x86_64-pc-windows-msvc/bin/codex.exe",
            &["vendor/x86_64-pc-windows-msvc/codex-path"],
        ),
        HostPlatform::WindowsArm64 => (
            "-win32-arm64",
            "vendor/aarch64-pc-windows-msvc/bin/codex.exe",
            &["vendor/aarch64-pc-windows-msvc/codex-path"],
        ),
        HostPlatform::MacArm64 => (
            "-darwin-arm64",
            "vendor/aarch64-apple-darwin/bin/codex",
            &["vendor/aarch64-apple-darwin/codex-path"],
        ),
        HostPlatform::MacX64 => (
            "-darwin-x64",
            "vendor/x86_64-apple-darwin/bin/codex",
            &["vendor/x86_64-apple-darwin/codex-path"],
        ),
        HostPlatform::Other => {
            return Distribution::Unsupported(UnsupportedReason::NoVendorBuild);
        }
    };
    Distribution::Supported(ArtifactPlan {
        feed: Feed::Npm {
            package: "@openai/codex",
            dist_tag: "latest",
            platform_suffix,
            versions: VersionFilter::Releases,
        },
        layout: Layout::TarTree {
            strip: "package/",
            entry,
            tool_dirs,
        },
        download_bound_bytes: 512 * MIB,
        expanded_bound_bytes: 1024 * MIB,
    })
}

const fn opencode2_distribution(platform: HostPlatform) -> Distribution {
    match platform {
        HostPlatform::WindowsX64 => Distribution::Supported(ArtifactPlan {
            feed: Feed::Npm {
                package: "@opencode-ai/cli-windows-x64",
                dist_tag: "beta",
                platform_suffix: "",
                versions: VersionFilter::Numbered("beta-"),
            },
            layout: Layout::TarMember {
                member: "package/bin/opencode2.exe",
                binary: "opencode2.exe",
            },
            download_bound_bytes: 256 * MIB,
            expanded_bound_bytes: 512 * MIB,
        }),
        _ => Distribution::Unsupported(UnsupportedReason::NoVendorBuild),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_engine_and_platform_has_a_verified_feed_or_an_explicit_reason() {
        for engine in ManagedEngine::ALL {
            assert_eq!(ManagedEngine::from_id(engine.id()), Some(engine));
            assert!(EngineVersion::parse(engine.floor()).is_some(), "{engine}");
            assert!(engine.override_env().starts_with("ARTISAN_"));
            for platform in HostPlatform::ALL {
                match engine.distribution(platform) {
                    Distribution::Supported(plan) => {
                        assert!(plan.download_bound_bytes > 0);
                        assert!(plan.expanded_bound_bytes >= plan.download_bound_bytes / 2);
                        let entry = plan.layout.entry();
                        assert!(!entry.is_empty() && !entry.starts_with('/'));
                        assert!(!entry.split('/').any(|part| part == ".." || part.is_empty()));
                        assert_eq!(
                            std::path::Path::new(entry)
                                .extension()
                                .is_some_and(|extension| extension.eq_ignore_ascii_case("exe")),
                            platform.is_windows(),
                            "{engine} {platform:?}"
                        );
                        match plan.feed {
                            Feed::ClaudeReleases { platform_key, .. } => {
                                assert_eq!(engine, ManagedEngine::Claude);
                                assert!(!platform_key.is_empty());
                            }
                            Feed::Npm {
                                package, dist_tag, ..
                            } => {
                                assert!(package.starts_with('@'));
                                assert!(!dist_tag.is_empty());
                            }
                        }
                    }
                    Distribution::Unsupported(reason) => {
                        assert!(!reason.code().is_empty());
                        assert!(!reason.message().is_empty());
                    }
                }
            }
        }
    }

    #[test]
    fn grok_and_cursor_are_unsupported_because_no_digest_is_published() {
        for engine in [ManagedEngine::Grok, ManagedEngine::Cursor] {
            for platform in HostPlatform::ALL {
                assert_eq!(
                    engine.distribution(platform),
                    Distribution::Unsupported(UnsupportedReason::NoVendorDigest)
                );
            }
        }
    }

    #[test]
    fn floors_match_the_launch_minimums() {
        let floor = |engine: ManagedEngine| EngineVersion::parse(engine.floor()).unwrap();
        assert_eq!(floor(ManagedEngine::Claude).as_str(), "2.1.220");
        assert!(ManagedEngine::Claude.meets_floor(&EngineVersion::parse("2.1.282").unwrap()));
        assert!(!ManagedEngine::Claude.meets_floor(&EngineVersion::parse("2.1.219").unwrap()));
        assert!(!ManagedEngine::Codex.meets_floor(&EngineVersion::parse("0.100.0").unwrap()));
        assert!(
            !ManagedEngine::OpenCode2.meets_floor(&EngineVersion::parse("0.0.0-beta-100").unwrap())
        );
    }

    #[test]
    fn version_filters_select_the_managed_channel() {
        let version = |value| EngineVersion::parse(value).unwrap();
        assert!(VersionFilter::Releases.accepts(&version("0.157.1")));
        assert!(!VersionFilter::Releases.accepts(&version("0.157.1-linux-x64")));
        assert!(!VersionFilter::Releases.accepts(&version("0.158.0-alpha.2")));
        let beta = VersionFilter::Numbered("beta-");
        assert!(beta.accepts(&version("0.0.0-beta-19271")));
        assert!(!beta.accepts(&version("0.0.0-dev-19272")));
        assert!(!beta.accepts(&version("1.18.18")));
    }
}
