//! Building the Debug stage with Cargo from the checkout.
//!
//! The Debug stage is the development loop, so it builds outside the Nix
//! sandbox: Cargo runs inside the checkout's `cargo-<platform>` dev shell
//! (the toolchain, libraries, linker, and cross environment of the stage's
//! Nix derivation, `nix/stages.nix`) with one persistent target directory per
//! build under `target/dev`, so an iteration recompiles only what changed,
//! incrementally. The binaries are then staged as a payload beside them with
//! the identity a Nix build would carry. Production stays a Nix build.

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use artisan_build_info::{BuildInfo, Channel, FORMAT_VERSION, RESOURCE_PATH};

use crate::{error::DevError, nix::Checkout, nix::Target};

/// Cargo profile of the Debug stage (`Cargo.toml`).
pub const PROFILE: &str = "production-debug";

/// Features of the Debug stage, as `nix/stages.nix` builds it.
pub const FEATURES: [&str; 3] = [
    "artisan-frontend/debug-tools",
    "artisan-frontend/flight-recorder",
    "artisan-backend/flight-recorder",
];

/// The four payload binaries and their packages, as `nix/stages.nix` builds
/// them.
pub const BINARIES: [(&str, &str); 4] = [
    ("artisan-frontend", "editor"),
    ("artisan-backend", "forge"),
    ("artisan-editor-cli", "ae"),
    ("ae-installer", "installer"),
];

/// Compile-time inputs that make the installer library a release build. The
/// runners carry them (the `dev` app exports them); payloads never do, like
/// the Debug payload derivation.
pub const RELEASE_TRUST_ENV: [&str; 3] = [
    "ARTISAN_INSTALLER_RELEASE",
    "ARTISAN_RELEASE_KEY_ID",
    "ARTISAN_RELEASE_PUBLIC_KEY_HEX",
];

/// One Cargo build of the dev loop.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Build {
    /// The four payload binaries for a platform.
    Payload(Target),
    /// A platform's dev runner.
    Runner(Target),
}

impl Build {
    const fn target(self) -> Target {
        match self {
            Self::Payload(target) | Self::Runner(target) => target,
        }
    }

    /// Directory under `target/dev` holding this build's Cargo state. Each
    /// build keeps its own, because the runners' release trust inputs would
    /// otherwise invalidate the payload's crates on every alternation.
    #[must_use]
    pub fn directory_name(self) -> String {
        match self {
            Self::Payload(target) => target.name().to_owned(),
            Self::Runner(target) => format!("{}-runner", target.name()),
        }
    }
}

/// The Rust target triple Cargo builds `target` for.
#[must_use]
pub fn target_triple(target: Target) -> String {
    match target {
        Target::Linux => format!("{}-unknown-linux-gnu", std::env::consts::ARCH),
        Target::Windows => "x86_64-pc-windows-gnu".to_owned(),
    }
}

/// The dev shell that builds for `target`, e.g. `cargo-windows`.
#[must_use]
pub fn shell_attribute(target: Target) -> String {
    format!("cargo-{}", target.name())
}

/// `name` as an executable file name for `target`.
#[must_use]
pub fn executable(target: Target, name: &str) -> String {
    match target {
        Target::Linux => name.to_owned(),
        Target::Windows => format!("{name}.exe"),
    }
}

/// Cargo state directory of `build` in the checkout.
#[must_use]
pub fn target_directory(checkout: &Checkout, build: Build) -> PathBuf {
    checkout
        .root
        .join("target")
        .join("dev")
        .join(build.directory_name())
}

/// Where Cargo leaves the binaries of `build`.
#[must_use]
pub fn output_directory(checkout: &Checkout, build: Build) -> PathBuf {
    target_directory(checkout, build)
        .join(target_triple(build.target()))
        .join(PROFILE)
}

/// The `cargo` arguments of `build`.
#[must_use]
pub fn cargo_arguments(checkout: &Checkout, build: Build) -> Vec<String> {
    let mut arguments = vec![
        "build".to_owned(),
        "--locked".to_owned(),
        "--profile".to_owned(),
        PROFILE.to_owned(),
        "--target".to_owned(),
        target_triple(build.target()),
        "--target-dir".to_owned(),
        target_directory(checkout, build).display().to_string(),
    ];
    match build {
        Build::Payload(_) => {
            for (package, binary) in BINARIES {
                arguments.extend(["-p", package, "--bin", binary].map(str::to_owned));
            }
            arguments.push("--features".to_owned());
            arguments.push(FEATURES.join(","));
        }
        Build::Runner(_) => {
            arguments.extend(["-p", "artisan-native-dev", "--bin", "dev"].map(str::to_owned));
        }
    }
    arguments
}

/// Runs `build` in its dev shell and returns the directory holding its
/// binaries. Cargo's progress goes to the runner's stderr.
///
/// # Errors
///
/// Returns [`DevError::Stage`] when Nix or Cargo fails.
pub fn build(checkout: &Checkout, build: Build) -> Result<PathBuf, DevError> {
    println!(
        "dev: building {} with cargo",
        build.directory_name().replace('-', " ")
    );
    let mut command = Command::new("nix");
    command
        .arg("develop")
        .arg(checkout.installable(&shell_attribute(build.target())))
        .args(["--command", "cargo"])
        .args(cargo_arguments(checkout, build))
        .current_dir(&checkout.root)
        // The default dev shell caps Cargo at two jobs for interactive use;
        // the dev loop gets every core.
        .env_remove("CARGO_BUILD_JOBS")
        .env_remove("CARGO_TARGET_DIR")
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    if let Build::Payload(_) = build {
        for name in RELEASE_TRUST_ENV {
            command.env_remove(name);
        }
    }
    let status = command
        .status()
        .map_err(|error| build_error(format!("cannot run nix develop: {error}")))?;
    if !status.success() {
        return Err(build_error(format!(
            "cargo build of {} failed ({status})",
            build.directory_name()
        )));
    }
    Ok(output_directory(checkout, build))
}

/// The version of a Cargo-built dev payload: the package version, the
/// commit count, and build metadata naming the commit, local changes, and the
/// build time, so every build is its own installable version (Nix builds use
/// their output hash instead).
#[must_use]
pub fn dev_version(
    package_version: &str,
    revision_count: u64,
    commit: Option<&str>,
    dirty: bool,
    built: u64,
) -> String {
    let mut metadata = Vec::new();
    if let Some(commit) = commit {
        metadata.push(format!("g{}", commit.get(..10).unwrap_or(commit)));
    }
    if dirty {
        metadata.push("dirty".to_owned());
    }
    metadata.push(format!("c{built}"));
    format!(
        "{package_version}-dev.{revision_count}+{}",
        metadata.join(".")
    )
}

/// The identity of a payload built from `checkout` for `target` now.
///
/// # Errors
///
/// Returns [`DevError::Stage`] when Git cannot describe the checkout.
pub fn identity(checkout: &Checkout, target: Target) -> Result<BuildInfo, DevError> {
    let commit = git(&checkout.root, &["rev-parse", "HEAD"])
        .ok()
        .map(|commit| commit.trim().to_owned())
        .filter(|commit| !commit.is_empty());
    let revision_count = git(&checkout.root, &["rev-list", "--count", "HEAD"])
        .ok()
        .and_then(|count| count.trim().parse().ok())
        .unwrap_or(0);
    let dirty = !git(
        &checkout.root,
        &["status", "--porcelain", "--untracked-files=no"],
    )?
    .trim()
    .is_empty();
    let built = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    Ok(BuildInfo {
        format_version: FORMAT_VERSION,
        version: dev_version(
            env!("CARGO_PKG_VERSION"),
            revision_count,
            commit.as_deref(),
            dirty,
            built,
        ),
        channel: Channel::Dev,
        commit,
        dirty,
        profile: PROFILE.to_owned(),
        target: target_triple(target),
        built_at: None,
    })
}

/// Stages the payload binaries in `binaries` for `target` as a payload
/// (`bin/` plus the identity) at `payload`, replacing a previous one. The
/// binaries are hard-linked where possible: Debug binaries exceed a gigabyte,
/// and the installer copies the payload anyway.
///
/// # Errors
///
/// Returns [`DevError::Stage`] when a binary is missing or the payload cannot
/// be written.
pub fn stage_payload(
    binaries: &Path,
    payload: &Path,
    target: Target,
    identity: &BuildInfo,
) -> Result<(), DevError> {
    let failed = |reason: String| DevError::Stage {
        stage: "payload",
        reason,
    };
    if payload.exists() {
        std::fs::remove_dir_all(payload)
            .map_err(|error| failed(format!("cannot clear {}: {error}", payload.display())))?;
    }
    let bin = payload.join("bin");
    std::fs::create_dir_all(&bin)
        .map_err(|error| failed(format!("cannot create {}: {error}", bin.display())))?;
    for (_, name) in BINARIES {
        let file = executable(target, name);
        let source = binaries.join(&file);
        let destination = bin.join(&file);
        if std::fs::hard_link(&source, &destination).is_err() {
            std::fs::copy(&source, &destination).map_err(|error| {
                failed(format!("cannot stage {}: {error}", source.display()))
            })?;
        }
    }
    let document = payload.join(RESOURCE_PATH);
    if let Some(resources) = document.parent() {
        std::fs::create_dir_all(resources)
            .map_err(|error| failed(format!("cannot create {}: {error}", resources.display())))?;
    }
    std::fs::write(&document, identity.to_json())
        .map_err(|error| failed(format!("cannot write {}: {error}", document.display())))
}

/// Builds the payload for `target` and stages it under its target
/// directory, returning the payload.
///
/// # Errors
///
/// Returns [`DevError::Stage`] when the build or staging fails.
pub fn payload(checkout: &Checkout, target: Target) -> Result<PathBuf, DevError> {
    let binaries = build(checkout, Build::Payload(target))?;
    let payload = target_directory(checkout, Build::Payload(target)).join("payload");
    stage_payload(
        &binaries,
        &payload,
        target,
        &identity(checkout, target)?,
    )?;
    Ok(payload)
}

/// Builds the dev runner for `target` and returns its executable.
///
/// # Errors
///
/// Returns [`DevError::Stage`] when the build fails.
pub fn runner(checkout: &Checkout, target: Target) -> Result<PathBuf, DevError> {
    Ok(build(checkout, Build::Runner(target))?.join(executable(target, "dev")))
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
