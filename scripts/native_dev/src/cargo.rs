//! Building the payload binaries with Cargo.
//!
//! The runner asks Cargo itself where the workspace and target directory
//! are, so `CARGO_TARGET_DIR`, configured target directories, and worktrees
//! all resolve the way Cargo resolves them.

use std::{path::PathBuf, process::Command};

use crate::error::DevError;

/// The four product binaries every payload carries, with their packages.
pub const PAYLOAD_BINARIES: [(&str, &str); 4] = [
    ("artisan-editor-cli", "ae"),
    ("artisan-backend", "forge"),
    ("artisan-frontend", "editor"),
    ("ae-installer", "installer"),
];

/// Where Cargo builds this workspace.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Workspace {
    /// Workspace root (the source checkout).
    pub root: PathBuf,
    /// Cargo target directory.
    pub target_directory: PathBuf,
}

impl Workspace {
    /// Locates the workspace containing the current directory.
    ///
    /// # Errors
    ///
    /// Returns [`DevError::Stage`] when Cargo cannot describe the workspace.
    pub fn locate() -> Result<Self, DevError> {
        let failed = |reason: String| DevError::Stage {
            stage: "build",
            reason,
        };
        let output = cargo_command()
            .args(["metadata", "--format-version", "1", "--no-deps", "--locked"])
            .output()
            .map_err(|error| failed(format!("cannot run cargo metadata: {error}")))?;
        if !output.status.success() {
            return Err(failed(format!(
                "cargo metadata failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        let metadata: serde_json::Value = serde_json::from_slice(&output.stdout)
            .map_err(|error| failed(format!("cargo metadata is not JSON: {error}")))?;
        let path = |key: &str| {
            metadata
                .get(key)
                .and_then(serde_json::Value::as_str)
                .map(PathBuf::from)
                .ok_or_else(|| failed(format!("cargo metadata has no {key}")))
        };
        Ok(Self {
            root: path("workspace_root")?,
            target_directory: path("target_directory")?,
        })
    }

    /// Output directory of `profile` (`dev` builds into `debug`).
    #[must_use]
    pub fn profile_directory(&self, profile: &str) -> PathBuf {
        self.target_directory.join(profile_directory_name(profile))
    }

    /// Builds the payload binaries with `profile` and returns their output
    /// directory. Cargo's output streams straight to the terminal.
    ///
    /// # Errors
    ///
    /// Returns [`DevError::Stage`] when Cargo cannot run or the build fails.
    pub fn build(&self, profile: &str) -> Result<PathBuf, DevError> {
        let mut command = cargo_command();
        command.current_dir(&self.root).args(["build", "--locked"]);
        for (package, binary) in PAYLOAD_BINARIES {
            command.args(["--package", package, "--bin", binary]);
        }
        command.args(["--profile", profile]);
        // Only release-type builds need the anchor. Injecting it into other
        // builds would change the installer crate's inputs relative to the
        // runner's own build of it, rebuilding it (with different bytes)
        // on every run.
        if self.profile_inherits_release(profile) {
            for (key, value) in self.release_trust_anchor() {
                if std::env::var_os(&key).is_none() {
                    command.env(key, value);
                }
            }
        }
        let status = command.status().map_err(|error| DevError::Stage {
            stage: "build",
            reason: format!("cannot run cargo build: {error}"),
        })?;
        if !status.success() {
            return Err(DevError::Stage {
                stage: "build",
                reason: format!("cargo build failed with {status}"),
            });
        }
        Ok(self.profile_directory(profile))
    }
}

impl Workspace {
    /// Whether `profile` is `release`, `bench`, or a workspace profile that
    /// inherits `release` (debug assertions off, so the installer compiles as
    /// a release build).
    fn profile_inherits_release(&self, profile: &str) -> bool {
        if matches!(profile, "release" | "bench") {
            return true;
        }
        let manifest = std::fs::read_to_string(self.root.join("Cargo.toml")).unwrap_or_default();
        let header = format!("[profile.{profile}]");
        manifest
            .lines()
            .skip_while(|line| line.trim() != header)
            .skip(1)
            .take_while(|line| !line.trim_start().starts_with('['))
            .any(|line| {
                line.split_once('=').is_some_and(|(key, value)| {
                    key.trim() == "inherits" && value.trim().trim_matches('"') == "release"
                })
            })
    }

    /// The checked-in pre-release trust anchor
    /// (`modules/installer/release/trust_anchor.env`). Release installer
    /// builds refuse to compile without an anchor (see `RELEASE_TRUST.md`),
    /// so local builds default to this one; an exported anchor always wins.
    /// Builds with debug assertions ignore it.
    fn release_trust_anchor(&self) -> Vec<(String, String)> {
        let path = self
            .root
            .join("modules")
            .join("installer")
            .join("release")
            .join("trust_anchor.env");
        std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| line.trim().split_once('='))
            .filter(|(key, _)| key.trim().starts_with("ARTISAN_RELEASE_"))
            .map(|(key, value)| {
                (
                    key.trim().to_owned(),
                    value.trim().trim_matches('"').to_owned(),
                )
            })
            .collect()
    }
}

/// Cargo output directory name of a profile.
#[must_use]
pub fn profile_directory_name(profile: &str) -> &str {
    match profile {
        "dev" | "test" => "debug",
        "bench" => "release",
        other => other,
    }
}

/// A Cargo command for this workspace, without the per-package variables
/// `cargo run` set for the runner itself.
///
/// Inheriting them would make the inner build's fingerprints disagree with
/// the outer `cargo run` (build scripts such as ring's track
/// `CARGO_MANIFEST_DIR`), so every run would rebuild the shared dependency
/// graph, and the product binaries with it. User configuration
/// (`CARGO_TARGET_DIR`, `CARGO_BUILD_JOBS`, profiles, ...) is kept.
fn cargo_command() -> Command {
    let mut command = Command::new(cargo());
    for (key, _) in std::env::vars_os() {
        let Some(key) = key.to_str() else {
            continue;
        };
        if is_cargo_run_variable(key) {
            command.env_remove(key);
        }
    }
    command
}

/// Whether `key` is one of the variables `cargo run` sets for the program
/// it runs rather than configuration the user chose.
#[must_use]
pub fn is_cargo_run_variable(key: &str) -> bool {
    key.starts_with("CARGO_PKG_")
        || matches!(
            key,
            "OUT_DIR"
                | "CARGO_MANIFEST_DIR"
                | "CARGO_MANIFEST_PATH"
                | "CARGO_MANIFEST_LINKS"
                | "CARGO_PRIMARY_PACKAGE"
                | "CARGO_CRATE_NAME"
                | "CARGO_BIN_NAME"
        )
}

/// The Cargo executable driving this run, falling back to `cargo` on PATH.
fn cargo() -> PathBuf {
    std::env::var_os("CARGO").map_or_else(|| PathBuf::from("cargo"), PathBuf::from)
}
