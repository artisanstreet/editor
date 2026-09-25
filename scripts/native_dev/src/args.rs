//! Command-line parsing for the native dev runner.
//!
//! Unknown commands and flags fail closed with usage text, so a typo never
//! installs half a payload.

use std::{ffi::OsString, path::PathBuf};

use crate::error::DevError;

/// Default number of inactive dev versions kept for rollback.
pub const DEFAULT_KEEP: usize = 3;

/// What the runner was asked to do.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Command {
    /// Install the payload and launch (or relaunch) the dev Editor.
    Run,
    /// Install the payload without launching.
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
    /// Nix-built payload to install (`run` and `stage`).
    pub payload: Option<PathBuf>,
    /// Inactive versions kept after an install or prune.
    pub keep: usize,
    /// Follow the launched Editor until it exits instead of returning once it
    /// confirms startup. A detached runner is what lets the next run replace
    /// the runner itself on Windows, where a running executable is locked.
    pub attach: bool,
}

impl DevArgs {
    /// Parses one argv slice (without the program name).
    ///
    /// # Errors
    ///
    /// Returns [`DevError::Usage`] for unknown commands, unknown flags,
    /// missing and malformed values, or `run`/`stage` without a payload.
    pub fn parse(argv: &[OsString]) -> Result<Action, DevError> {
        let mut options = Self {
            command: Command::Run,
            root: None,
            payload: None,
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
                "--payload" => options.payload = Some(PathBuf::from(value("--payload")?)),
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
        if matches!(options.command, Command::Run | Command::Stage) && options.payload.is_none() {
            return Err(usage(
                "run and stage install a Nix-built payload: pass --payload (or use `nix run .#dev`)"
                    .to_owned(),
            ));
        }
        Ok(Action::Execute(options))
    }
}

/// Usage text for `--help`.
#[must_use]
pub fn usage() -> &'static str {
    "usage: dev [run|stage] --payload PATH [--root PATH] [--keep N] [--attach]\n\
     \x20      dev [where|prune] [--root PATH] [--keep N]\n\
     \n\
     Installs a Nix-built payload as a signed dev-channel release into the\n\
     per-user `Artisan Street Dev` installation through the same installer code\n\
     releases use, and (for `run`) launches the dev Editor, closing any previous\n\
     dev Editor first. Normally driven by `nix run .#dev`, which builds the\n\
     payload first.\n\
     \n\
     run             install and launch or relaunch (default)\n\
     stage           install without launching\n\
     where           print the dev root, active version, and build identity\n\
     prune           remove superseded dev versions\n\
     \n\
     --payload PATH  Nix-built payload (bin/ and resources/build-info.json)\n\
     --root PATH     development installation root (default: per-user\n\
     \x20               `Artisan Street Dev`, or ARTISAN_DEV_ROOT)\n\
     --keep N        inactive versions kept for rollback (default: 3)\n\
     --attach        follow the launched Editor until it exits"
}
