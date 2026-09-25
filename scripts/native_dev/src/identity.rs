//! Build identity for locally staged payloads.
//!
//! The runner is the only place that knows both the source checkout and the
//! build it staged, so it derives the payload's [`BuildInfo`] here: the
//! checkout's commit and dirty state from Git, the Cargo profile from the
//! binary directory, and the compile target of this runner (which is built
//! for the same target as the payload it stages).

use std::{path::Path, process::Command};

/// Source checkout state at staging time.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GitState {
    /// Full `HEAD` commit hash.
    pub commit: Option<String>,
    /// Whether tracked files differ from `HEAD`.
    pub dirty: bool,
    /// Number of commits reachable from `HEAD`.
    pub commit_count: Option<u64>,
}

impl GitState {
    /// Reads the checkout containing `directory`. Without Git, or outside a
    /// checkout, every field is unknown rather than guessed.
    #[must_use]
    pub fn read(directory: &Path) -> Self {
        let git = |arguments: &[&str]| -> Option<String> {
            // The runner already builds and runs this checkout's code, so
            // trusting it for these read-only queries adds nothing; without
            // it Windows Git refuses checkouts on \\wsl.localhost as having
            // "dubious ownership" and every build would lose its commit.
            let output = Command::new("git")
                .args(["-c", "safe.directory=*", "-C"])
                .arg(directory)
                .args(arguments)
                .output()
                .ok()?;
            output
                .status
                .success()
                .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        };
        let Some(commit) = git(&["rev-parse", "HEAD"]).filter(|commit| !commit.is_empty()) else {
            return Self::default();
        };
        Self {
            commit: Some(commit),
            dirty: git(&["status", "--porcelain", "--untracked-files=no"])
                .is_some_and(|status| !status.is_empty()),
            commit_count: git(&["rev-list", "--count", "HEAD"])
                .and_then(|count| count.parse().ok()),
        }
    }
}

/// Cargo profile that produced the binaries in `bin_dir`, from Cargo's
/// output directory naming (`debug` is the `dev` profile).
#[must_use]
pub fn profile_for_bin_dir(bin_dir: &Path) -> String {
    match bin_dir.file_name().and_then(|name| name.to_str()) {
        Some("debug") | None => "dev".to_owned(),
        Some(profile) => profile.to_owned(),
    }
}

/// Compile target of this runner, recorded by its build script.
#[must_use]
pub const fn runner_target() -> &'static str {
    env!("ARTISAN_NATIVE_DEV_TARGET")
}

/// Semantic version for a local build: the package version with a `dev`
/// pre-release numbered by commit count, and build metadata naming the
/// commit and whether the checkout was dirty, e.g.
/// `0.0.0-dev.1284+g1a2b3c4d5e.dirty`.
#[must_use]
pub fn dev_version(package_version: &str, git: &GitState) -> String {
    let mut version = format!(
        "{package_version}-dev.{}",
        git.commit_count.unwrap_or_default()
    );
    let mut metadata = Vec::new();
    if let Some(commit) = &git.commit {
        metadata.push(format!("g{}", commit.get(..10).unwrap_or(commit)));
    }
    if git.dirty {
        metadata.push("dirty".to_owned());
    }
    if !metadata.is_empty() {
        version.push('+');
        version.push_str(&metadata.join("."));
    }
    version
}
