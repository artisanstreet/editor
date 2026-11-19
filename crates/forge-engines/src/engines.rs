use artisan_api::EngineId;
use std::cmp::Ordering;

pub(crate) const CLAUDE_RELEASES: &str = "https://downloads.claude.ai/claude-code-releases";
pub(crate) const NPM_REGISTRY: &str = "https://registry.npmjs.org";
const OPENCODE_REPOSITORY: &str = "artisanstreet/opencode";

#[derive(Clone, Copy, Debug)]
pub(crate) struct Spec {
    pub artifact: Option<Artifact>,
    pub minimum: Option<&'static str>,
    pub device_login: Option<&'static [&'static str]>,
    pub accepts_login_code: bool,
    pub logout: Option<&'static [&'static str]>,
    pub models: ModelSource,
    pub config: Option<(&'static str, &'static str)>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Artifact {
    pub feed: Feed,
    pub archive: Archive,
    pub strip: &'static str,
    pub entry: &'static str,
    pub path_dirs: &'static [&'static str],
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Feed {
    Claude {
        platform: &'static str,
        binary: &'static str,
    },
    Npm {
        package: &'static str,
        dist_tag: &'static str,
        suffix: &'static str,
    },
    Release {
        repository: &'static str,
        platform: &'static str,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Archive {
    Binary,
    TarGz,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModelSource {
    ClaudeCatalog,
    CodexAppServer,
    OpenCodeCli,
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
    fn current() -> Self {
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
const fn spec(engine: EngineId, platform: Platform) -> Spec {
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
            device_login: Some(&["auth", "login"]),
            accepts_login_code: true,
            logout: Some(&["auth", "logout"]),
            models: ModelSource::ClaudeCatalog,
            config: Some((".claude", "CLAUDE_CONFIG_DIR")),
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
                device_login: Some(&["login", "--device-auth"]),
                accepts_login_code: false,
                logout: Some(&["logout"]),
                models: ModelSource::CodexAppServer,
                config: Some((".codex", "CODEX_HOME")),
            }
        }
        EngineId::OpenCode => {
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
            }
        }
    }
}

pub(crate) fn host_spec(engine: EngineId) -> Spec {
    spec(engine, Platform::current())
}

#[must_use]
pub fn host_entry(engine: EngineId) -> Option<&'static str> {
    host_spec(engine).artifact.map(|artifact| artifact.entry)
}

pub(crate) fn compare_versions(left: &str, right: &str) -> Ordering {
    let parse = |value: &str| {
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

pub(crate) fn meets_minimum(engine: EngineId, version: &str) -> bool {
    host_spec(engine)
        .minimum
        .is_none_or(|minimum| compare_versions(version, minimum) != Ordering::Less)
}

pub(crate) fn environment(
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
#[path = "../tests/engines/reform.rs"]
mod reform_tests;
