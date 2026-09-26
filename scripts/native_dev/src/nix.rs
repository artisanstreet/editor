//! Building the stages with Nix from the checkout.
//!
//! The runner never compiles anything itself: it asks Nix for the stage
//! outputs of the checkout it was started in, in one `nix build`, so shared
//! dependencies build once and every output lands in the store. A dirty tree
//! builds its working copy of tracked files, which is why an untracked
//! source file refuses the build instead of silently missing from it.

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use crate::error::DevError;

/// The two build stages (Cargo profiles `production-debug` and
/// `production`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stage {
    /// Production codegen plus debug info, assertions, and the inspector.
    Debug,
    /// The shipped build.
    Production,
}

impl Stage {
    /// Flake attribute suffix.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Production => "production",
        }
    }
}

/// A platform the stages build for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Target {
    /// The Linux host (the Forge, and the Editor outside WSL).
    Linux,
    /// Windows, cross-built with MinGW-w64 (the Editor inside WSL).
    Windows,
}

impl Target {
    /// Flake attribute prefix.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Linux => "linux",
            Self::Windows => "windows",
        }
    }
}

/// The flake attribute of one stage payload, e.g. `linux-debug`.
#[must_use]
pub fn payload_attribute(target: Target, stage: Stage) -> String {
    format!("{}-{}", target.name(), stage.name())
}

/// The flake attribute of a platform's dev runner, e.g. `windows-runner`.
#[must_use]
pub fn runner_attribute(target: Target) -> String {
    format!("{}-runner", target.name())
}

/// The Git checkout whose flake the runner builds.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Checkout {
    /// Top-level directory of the working tree.
    pub root: PathBuf,
}

impl Checkout {
    /// The checkout containing `directory`.
    ///
    /// # Errors
    ///
    /// Returns [`DevError::Stage`] outside a Git checkout.
    pub fn locate(directory: &Path) -> Result<Self, DevError> {
        let root = git(directory, &["rev-parse", "--show-toplevel"]).map_err(|_| {
            build_error("run `nix run .#dev` inside the editor checkout".to_owned())
        })?;
        Ok(Self {
            root: PathBuf::from(root.trim()),
        })
    }

    /// Refuses to build while source files Nix cannot see are untracked.
    ///
    /// # Errors
    ///
    /// Returns [`DevError::Stage`] naming the untracked files.
    pub fn require_tracked_sources(&self) -> Result<(), DevError> {
        let untracked = git(
            &self.root,
            &[
                "ls-files",
                "--others",
                "--exclude-standard",
                "--",
                "*.rs",
                "*.toml",
                "*.nix",
            ],
        )?;
        untracked_refusal(&untracked).map_or(Ok(()), Err)
    }

    /// The installable for `attribute` of this checkout's flake.
    #[must_use]
    pub fn installable(&self, attribute: &str) -> String {
        format!("{}#{attribute}", self.root.display())
    }
}

/// The refusal for `git ls-files --others` output naming untracked sources.
#[must_use]
pub fn untracked_refusal(listing: &str) -> Option<DevError> {
    let files: Vec<&str> = listing.lines().filter(|line| !line.is_empty()).collect();
    if files.is_empty() {
        return None;
    }
    let shown = files.iter().take(5).copied().collect::<Vec<_>>().join(", ");
    Some(build_error(format!(
        "Nix only sees tracked files, and these are untracked: {shown}{}; track them with `git add -N <path>` and rerun",
        if files.len() > 5 { ", …" } else { "" }
    )))
}

/// Builds `installables` in one `nix build` and returns their output paths
/// in the same order. Nix's progress goes to the runner's stderr.
///
/// # Errors
///
/// Returns [`DevError::Stage`] when Nix fails or reports unexpected output.
pub fn build(installables: &[String]) -> Result<Vec<PathBuf>, DevError> {
    let output = Command::new("nix")
        // One derivation at a time: every payload ends in fat-LTO links, and
        // two payloads linking at once exhaust a 16 GB machine.
        .args(["build", "--no-link", "--json", "--max-jobs", "1"])
        .args(installables)
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output()
        .map_err(|error| build_error(format!("cannot run nix: {error}")))?;
    if !output.status.success() {
        return Err(build_error(format!("nix build failed ({})", output.status)));
    }
    let paths = parse_build_outputs(&String::from_utf8_lossy(&output.stdout))?;
    if paths.len() != installables.len() {
        return Err(build_error(format!(
            "nix built {} outputs for {} installables",
            paths.len(),
            installables.len()
        )));
    }
    Ok(paths)
}

/// Reads the `out` path of every result of `nix build --json`, in order.
///
/// # Errors
///
/// Returns [`DevError::Stage`] for output that is not the documented shape.
pub fn parse_build_outputs(json: &str) -> Result<Vec<PathBuf>, DevError> {
    let results: serde_json::Value = serde_json::from_str(json)
        .map_err(|error| build_error(format!("unreadable nix build output: {error}")))?;
    results
        .as_array()
        .ok_or_else(|| build_error("nix build output is not a list".to_owned()))?
        .iter()
        .map(|result| {
            result
                .get("outputs")
                .and_then(|outputs| outputs.get("out"))
                .and_then(serde_json::Value::as_str)
                .map(PathBuf::from)
                .ok_or_else(|| build_error("a nix build result has no `out` output".to_owned()))
        })
        .collect()
}

fn git(directory: &Path, arguments: &[&str]) -> Result<String, DevError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(arguments)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|error| build_error(format!("cannot run git: {error}")))?;
    if !output.status.success() {
        return Err(build_error(format!("git {} failed", arguments.join(" "))));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn build_error(reason: String) -> DevError {
    DevError::Stage {
        stage: "build",
        reason,
    }
}
