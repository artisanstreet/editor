use std::path::Path;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildInfo {
    pub version: String,
    pub commit: String,
    pub profile: String,
    pub target: String,
}

pub fn rows(build: &BuildInfo, executable: Option<&Path>) -> Vec<(&'static str, String)> {
    let mut rows = vec![("Version", build.version.clone())];
    rows.push((
        "Commit",
        if build.commit.is_empty() {
            "Unknown".to_owned()
        } else {
            build.commit.clone()
        },
    ));
    rows.push(("Profile", build.profile.clone()));
    rows.push(("Target", build.target.clone()));
    if let Some(executable) = executable {
        rows.push(("Executable", executable.display().to_string()));
    }
    rows
}
