//! Command-line parsing for the dev runner.
//!
//! `nix run .#dev` runs the Linux runner with the user's arguments. Inside
//! WSL it drives the Windows half by running the cross-built Windows runner
//! with the internal `editor` command group. Unknown commands and flags fail
//! closed with usage text, so a typo never installs half a build.

use std::{ffi::OsString, path::PathBuf};

use crate::{error::DevError, nix::Stage};

/// Default number of inactive dev versions kept for rollback.
pub const DEFAULT_KEEP: usize = 3;

/// What to do with the dev installations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Command {
    /// Build, install, and launch (or relaunch) the dev Editor.
    Run,
    /// Build and install without launching the Editor.
    Stage,
    /// Print the dev installations, active versions, and build identities.
    Where,
    /// Remove superseded dev versions.
    Prune,
}

/// Where the Editor runs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EditorPlatform {
    /// The Windows build, through WSL interop.
    Windows,
    /// The Linux build, from the same installation as the Forge.
    Linux,
}

/// A `nix run .#dev` invocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DevArgs {
    /// Requested command.
    pub command: Command,
    /// Build stage.
    pub stage: Stage,
    /// Editor platform; `None` picks Windows inside WSL, Linux otherwise.
    pub editor: Option<EditorPlatform>,
    /// Linux installation root; defaults to the per-user `Artisan Street Dev`.
    pub root: Option<PathBuf>,
    /// Windows installation root (a Windows path); defaults to
    /// `%LOCALAPPDATA%\Artisan Street Dev`.
    pub windows_root: Option<OsString>,
    /// Forge listening address (`IP:PORT` or `auto:PORT`).
    pub listen: Option<String>,
    /// Machine name the Forge's invitation carries.
    pub host_name: Option<String>,
    /// Inactive versions kept after an install or prune.
    pub keep: usize,
    /// Follow the launched Editor until it exits.
    pub attach: bool,
}

/// The internal Editor half, run on the Editor's platform.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditorArgs {
    /// Requested command.
    pub command: Command,
    /// Editor installation root; defaults to the per-user `Artisan Street Dev`.
    pub root: Option<PathBuf>,
    /// Payload to install (`run` and `stage`); absent when the Forge's
    /// installation already holds the Editor.
    pub payload: Option<PathBuf>,
    /// Host invitation to register (`run` and `stage`).
    pub invitation: Option<PathBuf>,
    /// Inactive versions kept after an install or prune.
    pub keep: usize,
    /// Follow the launched Editor until it exits.
    pub attach: bool,
}

/// What the argument parser decided.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Action {
    /// Print usage text.
    Help,
    /// Build and deploy both halves.
    Execute(DevArgs),
    /// Deploy the Editor half on this platform.
    Editor(EditorArgs),
}

fn usage_error(reason: String) -> DevError {
    DevError::Usage { reason }
}

fn command(name: &str) -> Result<Command, DevError> {
    match name {
        "run" => Ok(Command::Run),
        "stage" => Ok(Command::Stage),
        "where" => Ok(Command::Where),
        "prune" => Ok(Command::Prune),
        other => Err(usage_error(format!("unknown command `{other}`"))),
    }
}

/// Parses one argv slice (without the program name).
///
/// # Errors
///
/// Returns [`DevError::Usage`] for unknown commands and flags, missing or
/// malformed values, and an `editor run|stage` without payload or
/// invitation.
pub fn parse(argv: &[OsString]) -> Result<Action, DevError> {
    let mut words = argv.iter().peekable();
    let first = words
        .peek()
        .map(|word| word.to_string_lossy().into_owned())
        .filter(|word| !word.starts_with('-'));
    if first.as_deref() == Some("editor") {
        words.next();
        let name = words
            .next()
            .ok_or_else(|| usage_error("editor needs a command".to_owned()))?;
        return parse_editor(command(&name.to_string_lossy())?, words);
    }
    let mut options = DevArgs {
        command: Command::Run,
        stage: Stage::Debug,
        editor: None,
        root: None,
        windows_root: None,
        listen: None,
        host_name: None,
        keep: DEFAULT_KEEP,
        attach: false,
    };
    if let Some(name) = first {
        options.command = command(&name)?;
        words.next();
    }
    while let Some(flag) = words.next() {
        let mut value = |name: &str| {
            words
                .next()
                .cloned()
                .ok_or_else(|| usage_error(format!("{name} requires a value")))
        };
        match flag.to_string_lossy().as_ref() {
            "-h" | "--help" => return Ok(Action::Help),
            "--debug" => options.stage = Stage::Debug,
            "--production" => options.stage = Stage::Production,
            "--windows" => options.editor = Some(EditorPlatform::Windows),
            "--linux" => options.editor = Some(EditorPlatform::Linux),
            "--root" => options.root = Some(value("--root")?.into()),
            "--windows-root" => options.windows_root = Some(value("--windows-root")?),
            "--listen" => options.listen = Some(text(value("--listen")?, "--listen")?),
            "--host-name" => options.host_name = Some(text(value("--host-name")?, "--host-name")?),
            "--keep" => options.keep = keep(&value("--keep")?)?,
            "--attach" => options.attach = true,
            unknown => return Err(usage_error(format!("unknown flag `{unknown}`"))),
        }
    }
    Ok(Action::Execute(options))
}

fn parse_editor<'a>(
    command: Command,
    mut words: impl Iterator<Item = &'a OsString>,
) -> Result<Action, DevError> {
    let mut options = EditorArgs {
        command,
        root: None,
        payload: None,
        invitation: None,
        keep: DEFAULT_KEEP,
        attach: false,
    };
    while let Some(flag) = words.next() {
        let mut value = |name: &str| {
            words
                .next()
                .cloned()
                .ok_or_else(|| usage_error(format!("{name} requires a value")))
        };
        match flag.to_string_lossy().as_ref() {
            "--root" => options.root = Some(value("--root")?.into()),
            "--payload" => options.payload = Some(value("--payload")?.into()),
            "--invitation" => options.invitation = Some(value("--invitation")?.into()),
            "--keep" => options.keep = keep(&value("--keep")?)?,
            "--attach" => options.attach = true,
            unknown => return Err(usage_error(format!("unknown flag `{unknown}`"))),
        }
    }
    if matches!(options.command, Command::Run | Command::Stage) && options.invitation.is_none() {
        return Err(usage_error(
            "editor run and stage register a host: pass --invitation".to_owned(),
        ));
    }
    Ok(Action::Editor(options))
}

fn keep(value: &OsString) -> Result<usize, DevError> {
    value
        .to_string_lossy()
        .parse()
        .map_err(|_| usage_error("--keep requires a number".to_owned()))
}

fn text(value: OsString, name: &str) -> Result<String, DevError> {
    value
        .into_string()
        .map_err(|_| usage_error(format!("{name} must be valid Unicode")))
}

/// Usage text for `--help`.
#[must_use]
pub fn usage() -> &'static str {
    "usage: nix run .#dev -- [run|stage|where|prune] [options]\n\
     \n\
     Builds the Debug (or Production) stage with Nix and deploys both halves\n\
     through the shipping installer: the Linux installation, whose Forge runs\n\
     as the systemd user service artisan-forge-dev.service and publishes a\n\
     host invitation, and the Editor, registered with that host and launched.\n\
     Inside WSL the Editor is the Windows build, installed into\n\
     %LOCALAPPDATA%\\Artisan Street Dev through WSL interop. Rerunning\n\
     retires the running Forge and Editor the way an update does.\n\
     \n\
     run                 build, install, and launch or relaunch (default)\n\
     stage               build and install; the Forge runs, the Editor is not launched\n\
     where               print the installations, versions, and build identities\n\
     prune               remove superseded versions\n\
     \n\
     --production        the Production stage instead of Debug\n\
     --windows|--linux   Editor platform (default: Windows inside WSL, else Linux)\n\
     --root PATH         Linux installation root (default: $XDG_DATA_HOME/Artisan Street Dev)\n\
     --windows-root PATH Windows installation root (default: %LOCALAPPDATA%\\Artisan Street Dev)\n\
     --listen ADDRESS    Forge address, IP:PORT or auto:PORT (default: auto:4433 in WSL,\n\
     \x20                   127.0.0.1:4433 otherwise)\n\
     --host-name NAME    machine name Editors show (default: the WSL distribution or host name)\n\
     --keep N            inactive versions kept for rollback (default: 3)\n\
     --attach            follow the launched Editor until it exits"
}
