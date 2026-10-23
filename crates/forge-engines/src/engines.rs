//! The engine table: where each engine is published, how its download is
//! verified and laid out, the oldest version we run, how to log in and how
//! to list its models. Everything engine-specific about installs lives here.

use artisan_api::EngineId;
use std::cmp::Ordering;

pub const CLAUDE_RELEASES: &str = "https://downloads.claude.ai/claude-code-releases";
pub const NPM_REGISTRY: &str = "https://registry.npmjs.org";
/// Artisan's builds of OpenCode: upstream releases with the patches Forge needs.
pub const OPENCODE_REPOSITORY: &str = "artisanstreet/opencode";

/// One engine on one platform.
#[derive(Clone, Copy, Debug)]
pub struct Spec {
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
pub struct Artifact {
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
pub enum Feed {
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
    /// A GitHub repository's releases, tagged `artisan-v<version>`: the
    /// latest release's `manifest.json` names the version; a release's
    /// manifest carries the asset, SHA-256 and size per platform.
    Release {
        repository: &'static str,
        platform: &'static str,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Archive {
    /// The download is the executable.
    Binary,
    TarGz,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelSource {
    /// Anthropic's published catalog merged with the CLI's cached copy.
    ClaudeCatalog,
    /// `codex app-server` → `model/list`.
    CodexAppServer,
    /// `opencode models` plus its `models.json` cache for metadata.
    OpenCodeCli,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
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

#[expect(
    clippy::unnecessary_wraps,
    reason = "fills the `artifact` slot, which is None where an engine has no build"
)]
const fn single(feed: Feed, entry: &'static str) -> Option<Artifact> {
    Some(Artifact {
        feed,
        archive: Archive::Binary,
        strip: "",
        entry,
        path_dirs: &[],
    })
}

#[expect(clippy::too_many_lines, reason = "one entry per engine and platform")]
pub const fn spec(engine: EngineId, platform: Platform) -> Spec {
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
            // Artisan's builds of OpenCode 2's stable releases (`@opencode/cli`,
            // `latest`), packed like upstream's npm platform packages.
            let target = match platform {
                P::LinuxX64 => Some(("linux-x64", "bin/opencode")),
                P::LinuxX64Musl => Some(("linux-x64-musl", "bin/opencode")),
                P::LinuxArm64 => Some(("linux-arm64", "bin/opencode")),
                P::LinuxArm64Musl => Some(("linux-arm64-musl", "bin/opencode")),
                P::WindowsX64 => Some(("windows-x64", "bin/opencode.exe")),
                P::WindowsArm64 => Some(("windows-arm64", "bin/opencode.exe")),
                P::MacArm64 => Some(("darwin-arm64", "bin/opencode")),
                P::MacX64 => Some(("darwin-x64", "bin/opencode")),
                P::Other => None,
            };
            Spec {
                artifact: match target {
                    Some((platform, entry)) => Some(Artifact {
                        feed: Feed::Release {
                            repository: OPENCODE_REPOSITORY,
                            platform,
                        },
                        archive: Archive::TarGz,
                        strip: "package/",
                        entry,
                        path_dirs: &[],
                    }),
                    None => None,
                },
                minimum: Some("2.0.0"),
                login: &["auth", "login", "--standalone"],
                // OpenCode Console's device flow prints its link and code.
                device_login: Some(&[
                    "auth",
                    "login",
                    "opencode",
                    "--standalone",
                    "--method",
                    "device",
                ]),
                accepts_login_code: false,
                logout: None,
                models: ModelSource::OpenCodeCli,
                config: None,
                #[cfg(feature = "legacy-import")]
                legacy_dir: "opencode2",
            }
        }
    }
}

/// The table entry for this host.
pub fn host_spec(engine: EngineId) -> Spec {
    spec(engine, Platform::current())
}

/// SemVer precedence, with OpenCode's `beta-N` builds ordered by number.
pub fn compare_versions(left: &str, right: &str) -> Ordering {
    let parse = |value: &str| {
        // OpenCode's beta channel uses a single `beta-N` identifier for its
        // increasing build counter. Make that counter numeric for precedence.
        value
            .strip_prefix("0.0.0-beta-")
            .filter(|build| build.bytes().all(|byte| byte.is_ascii_digit()))
            .map_or_else(
                || semver::Version::parse(value),
                |build| semver::Version::parse(&format!("0.0.0-beta.{build}")),
            )
    };
    match (parse(left), parse(right)) {
        (Ok(left), Ok(right)) => left.cmp_precedence(&right),
        _ => left.cmp(right),
    }
}

pub fn meets_minimum(engine: EngineId, version: &str) -> bool {
    host_spec(engine)
        .minimum
        .is_none_or(|minimum| compare_versions(version, minimum) != Ordering::Less)
}

/// Environment shared by probes, login and runs: the engine's own state
/// pointed into its managed home and its tool directories ahead of `PATH`.
/// `HOME` and the XDG directories stay the user's, so the shells and tools an
/// engine runs find the user's own gh, git, ssh and toolchain configuration.
pub fn environment(
    engine: EngineId,
    binary: Option<&std::path::Path>,
    home: &std::path::Path,
) -> Vec<(String, String)> {
    let spec = host_spec(engine);
    let root = spec.artifact.zip(binary).and_then(|(artifact, binary)| {
        let mut root = binary;
        for _ in std::path::Path::new(artifact.entry).components() {
            root = root.parent()?;
        }
        Some(root)
    });
    let mut env = Vec::new();
    if let Some((directory, variable)) = spec.config {
        env.push((variable.into(), home.join(directory).display().to_string()));
    }
    match engine {
        EngineId::Claude => env.push(("DISABLE_UPDATES".into(), "1".into())),
        EngineId::Codex => {}
        // Artisan's OpenCode builds read OPENCODE_HOME; any other build only
        // moves with the XDG directories, which its tools then inherit.
        EngineId::OpenCode if root.is_some_and(understands_opencode_home) => {
            env.push(("OPENCODE_HOME".into(), home.display().to_string()));
        }
        EngineId::OpenCode => env.extend(
            [
                ("XDG_CONFIG_HOME", "config"),
                ("XDG_DATA_HOME", "data"),
                ("XDG_CACHE_HOME", "cache"),
                ("XDG_STATE_HOME", "state"),
            ]
            .map(|(key, directory)| (key.into(), home.join(directory).display().to_string())),
        ),
    }
    if let (Some(artifact), Some(root)) = (spec.artifact, root) {
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

/// Whether the OpenCode install at `root` is an Artisan build: its
/// `artisan.json` lists the `OPENCODE_HOME` feature.
fn understands_opencode_home(root: &std::path::Path) -> bool {
    std::fs::read(root.join("artisan.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|manifest| {
            manifest
                .get("features")?
                .as_array()?
                .iter()
                .any(|feature| feature.as_str() == Some("OPENCODE_HOME"))
                .then_some(())
        })
        .is_some()
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

    fn variables(env: &[(String, String)]) -> std::collections::HashMap<&str, &str> {
        env.iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect()
    }

    /// The shells and tools an engine runs see the user's own home (their
    /// gh login, git identity, ssh keys, toolchains); each engine's own
    /// state still lives in its managed home.
    #[test]
    fn engines_keep_their_state_in_the_managed_home_and_tools_see_the_users_home() {
        let home = std::path::Path::new("/data/engines/x/home");
        for engine in EngineId::ALL {
            let env = environment(engine, None, home);
            let env = variables(&env);
            let mut user = vec!["HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA"];
            // Without an install to inspect, OpenCode falls back to XDG
            // (covered below).
            if engine != EngineId::OpenCode {
                user.extend(["XDG_CONFIG_HOME", "XDG_DATA_HOME"]);
            }
            for variable in user {
                assert!(
                    !env.contains_key(variable),
                    "{engine:?} overrides {variable}"
                );
            }
        }
        let claude = environment(EngineId::Claude, None, home);
        assert_eq!(
            variables(&claude).get("CLAUDE_CONFIG_DIR").copied(),
            Some(home.join(".claude").to_str().unwrap())
        );
        let codex = environment(EngineId::Codex, None, home);
        assert_eq!(
            variables(&codex).get("CODEX_HOME").copied(),
            Some(home.join(".codex").to_str().unwrap())
        );
    }

    /// Artisan's OpenCode builds move with OPENCODE_HOME alone; a build
    /// without it (an install from before the fork) still keeps its sign-in
    /// in the managed home through the XDG directories.
    #[test]
    fn opencode_uses_its_own_home_variable_only_when_the_build_understands_it() {
        let Some(artifact) = host_spec(EngineId::OpenCode).artifact else {
            return;
        };
        let home = tempfile::tempdir().unwrap();
        let install = tempfile::tempdir().unwrap();
        let binary = install.path().join(artifact.entry);

        let upstream = environment(EngineId::OpenCode, Some(&binary), home.path());
        let upstream = variables(&upstream);
        assert!(!upstream.contains_key("OPENCODE_HOME"));
        assert_eq!(
            upstream.get("XDG_DATA_HOME").copied(),
            Some(home.path().join("data").to_str().unwrap())
        );

        std::fs::write(
            install.path().join("artisan.json"),
            r#"{"upstream":"v2.1.2","features":["OPENCODE_HOME"]}"#,
        )
        .unwrap();
        let artisan = environment(EngineId::OpenCode, Some(&binary), home.path());
        let artisan = variables(&artisan);
        assert_eq!(artisan.get("OPENCODE_HOME").copied(), home.path().to_str());
        assert!(
            !artisan.keys().any(|key| key.starts_with("XDG_")),
            "{artisan:?}"
        );
    }
}
