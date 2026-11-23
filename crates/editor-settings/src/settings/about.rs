use std::path::PathBuf;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildInfo {
    pub version: String,
    pub commit: String,
    pub profile: String,
    pub target: String,
    pub executable: Option<PathBuf>,
}

impl BuildInfo {
    pub fn identifier(&self) -> String {
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

    pub fn diagnostics(&self) -> String {
        format!(
            "Artisan Editor {}\nTarget: {}\nProfile: {}\nCommit: {}\n",
            self.identifier(),
            self.target,
            self.profile,
            self.commit
        )
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
