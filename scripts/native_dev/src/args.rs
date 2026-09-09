//! Command-line parsing for the native dev launcher.
//!
//! The launcher accepts exactly three flags; anything else fails closed
//! with usage text so a typo never stages half a home.

use std::{ffi::OsString, path::PathBuf};

use crate::error::DevError;

/// What the argument parser decided.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Action {
    /// Print usage text.
    Help,
    /// Run the dev stages.
    Run(DevArgs),
}

/// Parsed `dev` invocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DevArgs {
    /// Explicit dev directory; defaults to `<workspace>/.dist/dev`.
    pub dev_dir: Option<PathBuf>,
    /// Explicit directory holding prebuilt `ae`/`editor`/`forge`/`installer`
    /// binaries; defaults to the Bazel runfiles search.
    pub bin_dir: Option<PathBuf>,
    /// Stage and provision only; do not launch the Editor.
    pub stage_only: bool,
}

impl DevArgs {
    /// Parses one argv slice (without the program name).
    ///
    /// # Errors
    ///
    /// Returns [`DevError::Usage`] for unknown flags or missing values.
    pub fn parse(argv: &[OsString]) -> Result<Action, DevError> {
        let mut args = Self {
            dev_dir: None,
            bin_dir: None,
            stage_only: false,
        };
        let mut rest = argv.iter();
        while let Some(flag) = rest.next() {
            let flag = flag.to_string_lossy();
            match flag.as_ref() {
                "-h" | "--help" => return Ok(Action::Help),
                "--stage-only" => args.stage_only = true,
                "--dev-dir" => {
                    let value = rest.next().ok_or_else(|| DevError::Usage {
                        reason: "--dev-dir requires a path value".to_owned(),
                    })?;
                    args.dev_dir = Some(PathBuf::from(value));
                }
                "--bin-dir" => {
                    let value = rest.next().ok_or_else(|| DevError::Usage {
                        reason: "--bin-dir requires a path value".to_owned(),
                    })?;
                    args.bin_dir = Some(PathBuf::from(value));
                }
                unknown => {
                    return Err(DevError::Usage {
                        reason: format!("unknown flag `{unknown}`"),
                    });
                }
            }
        }
        Ok(Action::Run(args))
    }
}

/// Short usage text for `--help`.
#[must_use]
pub fn usage() -> &'static str {
    "usage: dev [--dev-dir PATH] [--bin-dir PATH] [--stage-only]\n\
     \n\
     Stage product binaries into <workspace>/.dist/dev, provision the\n\
     isolated dev home through the existing CLI custody APIs, and launch\n\
     the staged Editor on its newly owned Forge with startup confirmation.\n\
     \n\
     --dev-dir PATH  isolated installation root (default: <workspace>/.dist/dev)\n\
     --bin-dir PATH  directory with prebuilt ae/editor/forge/installer binaries\n\
     --stage-only    stage and provision without launching the Editor"
}
