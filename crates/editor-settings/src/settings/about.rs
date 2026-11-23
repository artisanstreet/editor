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
