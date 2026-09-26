//! Reaching the Windows side from WSL.
//!
//! Inside WSL the Editor is the Windows build: the Linux runner runs the
//! cross-built Windows runner through WSL interop, handing it Windows paths
//! of the Nix store payload and of the Linux Forge's host invitation. Paths
//! under a mounted drive (`/mnt/c/...`) become drive paths; everything else
//! is reached through the distribution's `\\wsl.localhost` share.

use std::{
    ffi::OsString,
    path::{Component, Path},
};

use crate::error::DevError;

/// What the Linux runner asks of the Windows runner.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditorInvocation<'a> {
    /// `run`, `stage`, `where`, or `prune`.
    pub command: &'a str,
    /// The Windows payload in the Nix store.
    pub payload: Option<&'a Path>,
    /// The Linux Forge's host invitation.
    pub invitation: Option<&'a Path>,
    /// Inactive versions kept.
    pub keep: usize,
    /// Windows installation root, already a Windows path.
    pub root: Option<&'a OsString>,
    /// Follow the launched Editor until it exits.
    pub attach: bool,
}

/// The Windows runner's `editor` arguments for `invocation`, with every
/// Linux path translated for `distribution`.
///
/// # Errors
///
/// Returns [`DevError::Stage`] for a path with no Windows form.
pub fn editor_arguments(
    invocation: &EditorInvocation<'_>,
    distribution: &str,
) -> Result<Vec<OsString>, DevError> {
    let mut arguments: Vec<OsString> = vec!["editor".into(), invocation.command.into()];
    for (flag, path) in [
        ("--payload", invocation.payload),
        ("--invitation", invocation.invitation),
    ] {
        if let Some(path) = path {
            arguments.push(flag.into());
            arguments.push(windows_path(path, distribution)?.into());
        }
    }
    arguments.push("--keep".into());
    arguments.push(invocation.keep.to_string().into());
    if let Some(root) = invocation.root {
        arguments.push("--root".into());
        arguments.push(root.clone());
    }
    if invocation.attach {
        arguments.push("--attach".into());
    }
    Ok(arguments)
}

/// Registrations of the WSL interop binfmt handler (the second is used when
/// systemd manages binfmt).
const INTEROP_REGISTRATIONS: [&str; 2] = [
    "/proc/sys/fs/binfmt_misc/WSLInterop",
    "/proc/sys/fs/binfmt_misc/WSLInterop-late",
];

/// Whether Windows executables can be run from this Linux system.
#[must_use]
pub fn interop_available() -> bool {
    INTEROP_REGISTRATIONS
        .iter()
        .any(|registration| Path::new(registration).exists())
}

/// The name of this WSL distribution (`$WSL_DISTRO_NAME`).
#[must_use]
pub fn distribution() -> Option<String> {
    std::env::var("WSL_DISTRO_NAME")
        .ok()
        .filter(|name| !name.is_empty())
}

/// The Windows path of the absolute Linux `path` in `distribution`.
///
/// # Errors
///
/// Returns [`DevError::Stage`] for a relative or non-UTF-8 path.
pub fn windows_path(path: &Path, distribution: &str) -> Result<String, DevError> {
    let invalid = || DevError::Stage {
        stage: "interop",
        reason: format!("{} has no Windows path", path.display()),
    };
    if !path.is_absolute() {
        return Err(invalid());
    }
    let parts = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_str().ok_or_else(invalid)),
            _ => None,
        })
        .collect::<Result<Vec<_>, _>>()?;
    if let ["mnt", drive, rest @ ..] = parts.as_slice()
        && drive.len() == 1
        && drive.chars().all(|letter| letter.is_ascii_alphabetic())
    {
        return Ok(format!(
            "{}:\\{}",
            drive.to_ascii_uppercase(),
            rest.join("\\")
        ));
    }
    Ok(format!(
        "\\\\wsl.localhost\\{distribution}\\{}",
        parts.join("\\")
    ))
}
