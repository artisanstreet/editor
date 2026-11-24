use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
#[error("write diagnostics to {}", path.display())]
pub struct WriteDiagnosticsError {
    pub path: PathBuf,
    #[source]
    pub source: std::io::Error,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildInfo {
    pub version: String,
    pub commit: String,
    pub profile: String,
    pub target: String,
    pub executable: Option<PathBuf>,
}

impl BuildInfo {
    pub(crate) fn identifier(&self) -> String {
        if self.version.is_empty() || self.version == "0.0.0" || self.version.contains("nightly") {
            format!(
                "N-{}",
                if self.commit.is_empty() {
                    "unknown"
                } else {
                    &self.commit
                }
            )
        } else {
            format!("S-{}", self.version)
        }
    }

    pub(crate) fn official(&self) -> bool {
        self.profile == "release"
    }

    pub(crate) fn description(&self) -> String {
        let mut parts = self.target.split('-');
        let arch = match parts.next() {
            Some("x86_64") => "x86-64",
            Some("aarch64") => "ARM64",
            Some(other) => other,
            None => "an unknown architecture",
        };
        let renderer = if self.target.contains("windows") {
            "D3D11"
        } else if self.target.contains("apple") {
            "Metal"
        } else {
            "Vulkan"
        };
        let toolchain = if self.target.ends_with("msvc") {
            "MSVC"
        } else if self.target.ends_with("gnu") || self.target.ends_with("gnullvm") {
            "GNU"
        } else {
            "LLVM"
        };
        format!(
            "Artisan Editor is using the {renderer} renderer and was compiled using {toolchain} for {arch}."
        )
    }

    pub fn diagnostics(&self, host: Option<&editor_core::HostData>) -> String {
        use std::fmt::Write;
        let mut report = format!(
            "Artisan Editor {}\nTarget: {}\nProfile: {}\nCommit: {}\n",
            self.identifier(),
            self.target,
            self.profile,
            self.commit
        );
        if let Some(host) = host {
            if let Some(version) = &host.forge_version {
                writeln!(report, "Forge: {version}")
                    .expect("invariant: writing diagnostics into a string cannot fail");
            }
            for (engine, status) in &host.engines {
                if let artisan_api::InstallState::Installed { version, .. } = &status.install {
                    writeln!(report, "{}: {version}", engine.display_name())
                        .expect("invariant: writing diagnostics into a string cannot fail");
                }
            }
        }
        report
    }
}

pub(crate) const LICENCES: [(&str, &str); 8] = [
    (
        "Aqua",
        include_str!("../../../editor-assets/licenses/aqua-BSD.txt"),
    ),
    (
        "Taffy",
        include_str!("../../../editor-assets/licenses/aqua-taffy-MIT.txt"),
    ),
    (
        "Spline Sans",
        include_str!("../../../editor-assets/licenses/spline-sans-OFL.txt"),
    ),
    (
        "Spline Sans Mono",
        include_str!("../../../editor-assets/licenses/spline-sans-mono-OFL.txt"),
    ),
    (
        "Tabler Icons",
        include_str!("../../../editor-assets/licenses/tabler-MIT.txt"),
    ),
    (
        "Simple Icons",
        include_str!("../../../editor-assets/licenses/simple-icons-CC0.txt"),
    ),
    (
        "Lobe Icons",
        include_str!("../../../editor-assets/licenses/lobe-icons-MIT.txt"),
    ),
    (
        "SVGL",
        include_str!("../../../editor-assets/licenses/svgl-svelte-LICENSE.txt"),
    ),
];
