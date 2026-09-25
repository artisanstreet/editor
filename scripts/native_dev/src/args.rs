//! Command-line parsing for the native dev runner.
//!
//! Unknown commands and flags fail closed with usage text, so a typo never
//! builds or installs half a payload.

use std::{ffi::OsString, path::PathBuf};

use crate::error::DevError;

/// Default number of inactive dev versions kept for rollback.
pub const DEFAULT_KEEP: usize = 3;

/// What the runner was asked to do.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Command {
    /// Build, install, and launch (or relaunch) the dev Editor.
    Run,
    /// Build and install without launching.
    Stage,
    /// Print the dev root, active version, and build identity.
    Where,
    /// Remove superseded dev versions.
    Prune,
}

/// What the argument parser decided.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Action {
    /// Print usage text.
    Help,
    /// Execute one command.
    Execute(DevArgs),
}

/// Parsed `dev` invocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DevArgs {
    /// Requested command.
    pub command: Command,
    /// Explicit development root; defaults to the per-user `Artisan Street Dev`.
    pub root: Option<PathBuf>,
    /// Cargo profile to build, or that prebuilt `--bin-dir` binaries were
    /// built with; `None` means `dev` (or, for `--bin-dir`, the profile
    /// named by the Cargo output directory).
    pub profile: Option<String>,
    /// Prebuilt binaries to install instead of building.
    pub bin_dir: Option<PathBuf>,
    /// Inactive versions kept after an install or prune.
    pub keep: usize,
    /// Follow the launched Editor until it exits instead of returning once it
    /// confirms startup. A detached runner is what lets the next run rebuild
    /// the runner itself on Windows, where a running executable is locked.
    pub attach: bool,
}

impl DevArgs {
    /// Parses one argv slice (without the program name).
    ///
    /// # Errors
    ///
    /// Returns [`DevError::Usage`] for unknown commands, unknown flags, or
    /// missing and malformed values.
    pub fn parse(argv: &[OsString]) -> Result<Action, DevError> {
        let mut options = Self {
            command: Command::Run,
            root: None,
            profile: None,
            bin_dir: None,
            keep: DEFAULT_KEEP,
            attach: false,
        };
        let usage = |reason: String| DevError::Usage { reason };
        let mut rest = argv.iter().peekable();
        if let Some(first) = rest
            .peek()
            .map(|value| value.to_string_lossy().into_owned())
            && !first.starts_with('-')
        {
            options.command = match first.as_str() {
                "run" => Command::Run,
                "stage" => Command::Stage,
                "where" => Command::Where,
                "prune" => Command::Prune,
                other => return Err(usage(format!("unknown command `{other}`"))),
            };
            rest.next();
        }
        while let Some(flag) = rest.next() {
            let flag = flag.to_string_lossy();
            let mut value = |name: &str| {
                rest.next()
                    .ok_or_else(|| usage(format!("{name} requires a value")))
            };
            match flag.as_ref() {
                "-h" | "--help" => return Ok(Action::Help),
                "--root" => options.root = Some(PathBuf::from(value("--root")?)),
                "--bin-dir" => options.bin_dir = Some(PathBuf::from(value("--bin-dir")?)),
                "--profile" => {
                    options.profile = Some(value("--profile")?.to_string_lossy().into_owned());
                }
                "--release" => options.profile = Some("release".to_owned()),
                "--attach" => options.attach = true,
                "--keep" => {
                    options.keep = value("--keep")?
                        .to_string_lossy()
                        .parse()
                        .map_err(|_| usage("--keep requires a number".to_owned()))?;
                }
                unknown => return Err(usage(format!("unknown flag `{unknown}`"))),
            }
        }
        if let Some(profile) = &options.profile
            && (profile.is_empty()
                || !profile.chars().all(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
                }))
        {
            return Err(usage(format!("invalid profile `{profile}`")));
        }
        Ok(Action::Execute(options))
    }
}

/// Usage text for `--help`.
#[must_use]
pub fn usage() -> &'static str {
    "usage: cargo dev [run|stage|where|prune] [--root PATH] [--profile NAME | --release]\n\
     \x20                [--bin-dir PATH] [--keep N] [--attach]\n\
     \n\
     Builds the Artisan binaries, installs them as a signed dev-channel\n\
     release into the per-user `Artisan Street Dev` installation through the\n\
     same installer code releases use, and (for `run`) launches the dev\n\
     Editor, closing any previous dev Editor first.\n\
     \n\
     run             build, install, and launch or relaunch (default)\n\
     stage           build and install without launching\n\
     where           print the dev root, active version, and build identity\n\
     prune           remove superseded dev versions\n\
     \n\
     --root PATH     development installation root (default: per-user\n\
     \x20               `Artisan Street Dev`, or ARTISAN_DEV_ROOT)\n\
     --profile NAME  Cargo profile to build (default: dev); with --bin-dir,\n\
     \x20               the profile the binaries were built with\n\
     --release       shorthand for --profile release\n\
     --bin-dir PATH  install prebuilt ae/editor/forge/installer binaries\n\
     --keep N        inactive versions kept for rollback (default: 3)\n\
     --attach        follow the launched Editor until it exits"
}
