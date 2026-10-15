//! The engine table: where each engine is published, how its download is
//! verified and laid out, the oldest version we run, how to log in and how
//! to list its models. Everything engine-specific about installs lives here.

use artisan_api::EngineId;
use std::cmp::Ordering;

pub(crate) const CLAUDE_RELEASES: &str = "https://downloads.claude.ai/claude-code-releases";
pub(crate) const NPM_REGISTRY: &str = "https://registry.npmjs.org";
pub(crate) const GROK_RELEASES: &str = "https://x.ai/cli";
pub(crate) const CURSOR_INSTALLER: &str = "https://cursor.com/install";
pub(crate) const CURSOR_RELEASES: &str = "https://downloads.cursor.com/lab";

/// One engine on one platform.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Spec {
    /// `None` when the vendor publishes no build for this platform.
    pub artifact: Option<Artifact>,
    /// Oldest version we install or run.
    pub minimum: Option<&'static str>,
    /// Arguments that start the engine's own sign-in flow.
    pub login: &'static [&'static str],
    /// Noninteractive paired-client sign-in, distinct from CLI interactive login.
    pub device_login: Option<&'static [&'static str]>,
    pub accepts_login_code: bool,
    pub logout: Option<&'static [&'static str]>,
    pub models: ModelSource,
    /// The engine's config directory inside its home and the variable that
    /// points the engine at it, for engines that read one.
    pub config: Option<(&'static str, &'static str)>,
    /// Directory name under the old `toolchain/`.
    #[cfg(feature = "legacy-import")]
    pub legacy_dir: &'static str,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Artifact {
    pub feed: Feed,
    pub archive: Archive,
    /// Path prefix inside the archive that becomes the version directory.
    pub strip: &'static str,
    /// The executable, relative to the version directory.
    pub entry: &'static str,
    /// Directories (relative to the version directory) the engine expects
    /// on its `PATH`.
    pub path_dirs: &'static [&'static str],
}

/// Where versions and downloads come from. The feed also decides the
/// verification: whatever digest the vendor publishes, nothing more.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Feed {
    /// `<base>/latest` names the version; `<base>/<v>/manifest.json` carries
    /// SHA-256 and size per platform; `<base>/<v>/<platform>/<binary>`.
    Claude {
        platform: &'static str,
        binary: &'static str,
    },
    /// npm: the dist-tag names the version; the version document carries the
    /// tarball URL and its SHA-512 `integrity`.
    Npm {
        package: &'static str,
        dist_tag: &'static str,
        suffix: &'static str,
    },
    /// `<base>/stable` names the version; `<base>/grok-<v>-<platform>`.
    /// No digest is published: HTTPS only.
    Grok { platform: &'static str, exe: bool },
    /// The installer script names the version;
    /// `<base>/<v>/<os>/<arch>/agent-cli-package.tar.gz`. No digest: HTTPS only.
    Cursor {
        os: &'static str,
        arch: &'static str,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Archive {
    /// The download is the executable.
    Binary,
    TarGz,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModelSource {
    /// Anthropic's published catalog merged with the CLI's cached copy.
    ClaudeCatalog,
    /// `codex app-server` → `model/list`.
    CodexAppServer,
    /// `opencode2 models` plus its `models.json` cache for metadata.
    OpenCodeCli,
    /// A CLI subcommand that prints one model per line.
    CliLines(&'static [&'static str]),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Platform {
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

impl Platform {
    /// Engines are separate executables, so on Linux the C library comes
    /// from the host's loader, not from how Forge was built.
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
}

const fn single(feed: Feed, entry: &'static str) -> Option<Artifact> {
    Some(Artifact {
        feed,
        archive: Archive::Binary,
        strip: "",
        entry,
        path_dirs: &[],
    })
}

#[allow(clippy::too_many_lines)]
pub(crate) const fn spec(engine: EngineId, platform: Platform) -> Spec {
    use Platform as P;
    match engine {
        EngineId::Claude => Spec {
            artifact: match platform {
                P::LinuxX64 => single(
                    Feed::Claude {
                        platform: "linux-x64",
                        binary: "claude",
                    },
                    "claude",
                ),
                P::LinuxX64Musl => single(
                    Feed::Claude {
                        platform: "linux-x64-musl",
                        binary: "claude",
                    },
                    "claude",
                ),
                P::LinuxArm64 => single(
                    Feed::Claude {
                        platform: "linux-arm64",
                        binary: "claude",
                    },
                    "claude",
                ),
                P::LinuxArm64Musl => single(
                    Feed::Claude {
                        platform: "linux-arm64-musl",
                        binary: "claude",
                    },
                    "claude",
                ),
                P::WindowsX64 => single(
                    Feed::Claude {
                        platform: "win32-x64",
                        binary: "claude.exe",
                    },
                    "claude.exe",
                ),
                P::WindowsArm64 => single(
                    Feed::Claude {
                        platform: "win32-arm64",
                        binary: "claude.exe",
                    },
                    "claude.exe",
                ),
                P::MacArm64 => single(
                    Feed::Claude {
                        platform: "darwin-arm64",
                        binary: "claude",
                    },
                    "claude",
                ),
                P::MacX64 => single(
                    Feed::Claude {
                        platform: "darwin-x64",
                        binary: "claude",
                    },
                    "claude",
                ),
                P::Other => None,
            },
            minimum: Some("2.1.220"),
            login: &["auth", "login"],
            device_login: Some(&["auth", "login"]),
            accepts_login_code: true,
            logout: Some(&["auth", "logout"]),
            models: ModelSource::ClaudeCatalog,
            config: Some((".claude", "CLAUDE_CONFIG_DIR")),
            #[cfg(feature = "legacy-import")]
            legacy_dir: "claude",
        },
        EngineId::Codex => {
            let target: Option<(&str, &str, &[&str])> = match platform {
                P::LinuxX64 | P::LinuxX64Musl => Some((
                    "-linux-x64",
                    "vendor/x86_64-unknown-linux-musl/bin/codex",
                    &["vendor/x86_64-unknown-linux-musl/codex-path"],
                )),
                P::LinuxArm64 | P::LinuxArm64Musl => Some((
                    "-linux-arm64",
                    "vendor/aarch64-unknown-linux-musl/bin/codex",
                    &["vendor/aarch64-unknown-linux-musl/codex-path"],
                )),
                P::WindowsX64 => Some((
                    "-win32-x64",
                    "vendor/x86_64-pc-windows-msvc/bin/codex.exe",
                    &["vendor/x86_64-pc-windows-msvc/codex-path"],
                )),
                P::WindowsArm64 => Some((
                    "-win32-arm64",
                    "vendor/aarch64-pc-windows-msvc/bin/codex.exe",
                    &["vendor/aarch64-pc-windows-msvc/codex-path"],
                )),
                P::MacArm64 => Some((
                    "-darwin-arm64",
                    "vendor/aarch64-apple-darwin/bin/codex",
                    &["vendor/aarch64-apple-darwin/codex-path"],
                )),
                P::MacX64 => Some((
                    "-darwin-x64",
                    "vendor/x86_64-apple-darwin/bin/codex",
                    &["vendor/x86_64-apple-darwin/codex-path"],
                )),
                P::Other => None,
            };
            Spec {
                artifact: match target {
                    Some((suffix, entry, path_dirs)) => Some(Artifact {
                        feed: Feed::Npm {
                            package: "@openai/codex",
                            dist_tag: "latest",
                            suffix,
                        },
                        archive: Archive::TarGz,
                        strip: "package/",
                        entry,
                        path_dirs,
                    }),
                    None => None,
                },
                minimum: Some("0.142.5"),
                login: &["login"],
                device_login: Some(&["login", "--device-auth"]),
                accepts_login_code: false,
                logout: Some(&["logout"]),
                models: ModelSource::CodexAppServer,
                config: Some((".codex", "CODEX_HOME")),
                #[cfg(feature = "legacy-import")]
                legacy_dir: "codex",
            }
        }
        EngineId::OpenCode => {
            // The old installer managed only Windows; the same npm packages
            // exist for Linux and macOS (`package/bin/opencode2` checked on
            // linux-x64, beta-19271).
            let target = match platform {
                P::LinuxX64 | P::LinuxX64Musl => {
                    Some(("@opencode-ai/cli-linux-x64", "bin/opencode2"))
                }
                P::LinuxArm64 | P::LinuxArm64Musl => {
                    Some(("@opencode-ai/cli-linux-arm64", "bin/opencode2"))
                }
                P::WindowsX64 => Some(("@opencode-ai/cli-windows-x64", "bin/opencode2.exe")),
                P::MacArm64 => Some(("@opencode-ai/cli-darwin-arm64", "bin/opencode2")),
                P::WindowsArm64 | P::MacX64 | P::Other => None,
            };
            Spec {
                artifact: match target {
                    Some((package, entry)) => Some(Artifact {
                        feed: Feed::Npm {
                            package,
                            dist_tag: "beta",
                            suffix: "",
                        },
                        archive: Archive::TarGz,
                        strip: "package/",
                        entry,
                        path_dirs: &[],
                    }),
                    None => None,
                },
                minimum: Some("0.0.0-beta-17778"),
                login: &["auth", "login"],
                device_login: None,
                accepts_login_code: false,
                logout: None,
                models: ModelSource::OpenCodeCli,
                config: None,
                #[cfg(feature = "legacy-import")]
                legacy_dir: "opencode2",
            }
        }
        EngineId::Grok => Spec {
            artifact: match platform {
                P::LinuxX64 | P::LinuxX64Musl => single(
                    Feed::Grok {
                        platform: "linux-x86_64",
                        exe: false,
                    },
                    "grok",
                ),
                P::LinuxArm64 | P::LinuxArm64Musl => single(
                    Feed::Grok {
                        platform: "linux-aarch64",
                        exe: false,
                    },
                    "grok",
                ),
                P::WindowsX64 => single(
                    Feed::Grok {
                        platform: "windows-x86_64",
                        exe: true,
                    },
                    "grok.exe",
                ),
                P::MacArm64 => single(
                    Feed::Grok {
                        platform: "macos-aarch64",
                        exe: false,
                    },
                    "grok",
                ),
                P::MacX64 => single(
                    Feed::Grok {
                        platform: "macos-x86_64",
                        exe: false,
                    },
                    "grok",
                ),
                P::WindowsArm64 | P::Other => None,
            },
            minimum: None,
            login: &["login"],
            device_login: Some(&["login", "--device-auth"]),
            accepts_login_code: false,
            logout: Some(&["logout"]),
            models: ModelSource::CliLines(&["--no-auto-update", "models"]),
            config: None,
            #[cfg(feature = "legacy-import")]
            legacy_dir: "grok",
        },
        EngineId::Cursor => {
            // Cursor ships Windows as a zip whose layout we haven't seen.
            let target = match platform {
                P::LinuxX64 | P::LinuxX64Musl => Some(("linux", "x64")),
                P::LinuxArm64 | P::LinuxArm64Musl => Some(("linux", "arm64")),
                P::MacArm64 => Some(("darwin", "arm64")),
                P::MacX64 => Some(("darwin", "x64")),
                P::WindowsX64 | P::WindowsArm64 | P::Other => None,
            };
            Spec {
                artifact: match target {
                    Some((os, arch)) => Some(Artifact {
                        feed: Feed::Cursor { os, arch },
                        archive: Archive::TarGz,
                        strip: "dist-package/",
                        entry: "cursor-agent",
                        path_dirs: &[],
                    }),
                    None => None,
                },
                minimum: None,
                login: &["login"],
                device_login: Some(&["login"]),
                accepts_login_code: false,
                logout: Some(&["logout"]),
                models: ModelSource::CliLines(&["models"]),
                config: None,
                #[cfg(feature = "legacy-import")]
                legacy_dir: "cursor",
            }
        }
    }
}

/// The table entry for this host.
pub(crate) fn host_spec(engine: EngineId) -> Spec {
    spec(engine, Platform::current())
}

/// SemVer precedence for providers using SemVer; Cursor releases use an
/// ISO date followed by a commit hash, ordered by the fixed-width date.
pub(crate) fn compare_versions(left: &str, right: &str) -> Ordering {
    let parse = |value: &str| {
        // OpenCode's beta channel uses a single `beta-N` identifier for its
        // increasing build counter. Make that counter numeric for precedence.
        if let Some(build) = value
            .strip_prefix("0.0.0-beta-")
            .filter(|build| build.bytes().all(|byte| byte.is_ascii_digit()))
        {
            semver::Version::parse(&format!("0.0.0-beta.{build}"))
        } else {
            semver::Version::parse(value)
        }
    };
    match (parse(left), parse(right)) {
        (Ok(left), Ok(right)) => left.cmp_precedence(&right),
        _ => left.cmp(right),
    }
}

pub(crate) fn meets_minimum(engine: EngineId, version: &str) -> bool {
    host_spec(engine)
        .minimum
        .is_none_or(|minimum| compare_versions(version, minimum) != Ordering::Less)
}

/// Managed home and provider tool environment shared by probes, login and runs.
pub(crate) fn environment(
    engine: EngineId,
    binary: Option<&std::path::Path>,
    home: &std::path::Path,
) -> Vec<(String, String)> {
    let mut env = vec![("HOME".into(), home.display().to_string())];
    if cfg!(windows) {
        env.extend([
            ("USERPROFILE".into(), home.display().to_string()),
            (
                "APPDATA".into(),
                home.join("AppData/Roaming").display().to_string(),
            ),
            (
                "LOCALAPPDATA".into(),
                home.join("AppData/Local").display().to_string(),
            ),
        ]);
    }
    let spec = host_spec(engine);
    if let Some((directory, variable)) = spec.config {
        env.push((variable.into(), home.join(directory).display().to_string()));
    }
    if engine == EngineId::Claude {
        env.push(("DISABLE_UPDATES".into(), "1".into()));
    }
    if engine == EngineId::OpenCode {
        env.extend(
            [
                ("XDG_CONFIG_HOME", "config"),
                ("XDG_DATA_HOME", "data"),
                ("XDG_CACHE_HOME", "cache"),
                ("XDG_STATE_HOME", "state"),
            ]
            .map(|(key, directory)| (key.into(), home.join(directory).display().to_string())),
        );
    }
    if let (Some(artifact), Some(binary)) = (spec.artifact, binary) {
        let mut root = binary;
        for _ in std::path::Path::new(artifact.entry).components() {
            let Some(parent) = root.parent() else {
                return env;
            };
            root = parent;
        }
        let inherited = std::env::var_os("PATH").unwrap_or_default();
        let directories = artifact
            .path_dirs
            .iter()
            .map(|directory| root.join(directory));
        if let Ok(path) = std::env::join_paths(directories.chain(std::env::split_paths(&inherited)))
        {
            env.push(("PATH".into(), path.to_string_lossy().into_owned()));
        }
    }
    env
}

#[cfg(test)]
mod reform_tests {
    use super::*;
    #[test]
    fn published_version_precedence_matches_provider_formats() {
        assert_eq!(compare_versions("1.0.0", "1.0.0-beta.2"), Ordering::Greater);
        assert_eq!(compare_versions("1.0.0+one", "1.0.0+two"), Ordering::Equal);
        assert_eq!(
            compare_versions("0.0.0-beta-19271", "0.0.0-beta-19270"),
            Ordering::Greater
        );
        assert_eq!(
            compare_versions("2026.10.01-e373342", "2026.09.30-acde"),
            Ordering::Greater
        );
    }
}
