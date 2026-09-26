//! Windows autostart: the current user's Forge logon task.
//!
//! Linux starts the Forge through its systemd user service instead; there
//! this module is compiled only for its tests.

#![cfg_attr(
    target_os = "linux",
    expect(
        dead_code,
        reason = "Linux autostart is the systemd user service; the logon task is only tested there"
    )
)]

use std::path::{Path, PathBuf};
#[cfg(target_os = "windows")]
use std::process::{Command, Stdio};

use crate::{CliError, Result, paths::Layout};

#[cfg(target_os = "windows")]
use crate::error::io;

use super::require_installation;

const AUTOSTART_TASK_NAME: &str = "Artisan Forge";

pub(super) fn enable(layout: &Layout) -> Result<()> {
    let manifest = require_installation(layout)?;
    let permanent_ae = manifest.permanent_ae_path.as_deref().ok_or_else(|| {
        CliError::Installation(
            "the installation has no permanent ae launcher path; run `ae doctor --fix` before enabling autostart"
                .into(),
        )
    })?;
    let launcher = stable_launcher_kind(permanent_ae).ok_or_else(|| {
        CliError::Installation(format!(
            "the permanent ae launcher at {} must be an absolute ae.exe, ae.cmd, or ae.bat file; run `ae doctor --fix` before enabling autostart",
            permanent_ae.display()
        ))
    })?;
    if !permanent_ae.is_absolute() || !permanent_ae.is_file() {
        return Err(CliError::Installation(format!(
            "the permanent ae launcher is unavailable at {}; run `ae doctor --fix` before enabling autostart",
            permanent_ae.display()
        )));
    }
    let action = scheduled_task_action(
        permanent_ae,
        launcher,
        match launcher {
            StableLauncher::Executable => None,
            StableLauncher::CommandScript => Some(trusted_windows_command_processor()?),
        },
    )?;
    create_autostart_task(&action)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::commands) enum StableLauncher {
    Executable,
    CommandScript,
}

pub(in crate::commands) fn stable_launcher_kind(path: &Path) -> Option<StableLauncher> {
    match path.file_name()?.to_str()? {
        name if name.eq_ignore_ascii_case("ae.exe") => Some(StableLauncher::Executable),
        name if name.eq_ignore_ascii_case("ae.cmd") || name.eq_ignore_ascii_case("ae.bat") => {
            Some(StableLauncher::CommandScript)
        }
        _ => None,
    }
}

pub(in crate::commands) fn scheduled_task_action(
    permanent_ae: &Path,
    launcher: StableLauncher,
    command_processor: Option<PathBuf>,
) -> Result<String> {
    match launcher {
        StableLauncher::Executable => Ok(format!("\"{}\" start", permanent_ae.display())),
        StableLauncher::CommandScript => {
            let command_processor = command_processor.ok_or_else(|| {
                CliError::Installation(
                    "no trusted Windows command processor is available for the permanent ae script"
                        .into(),
                )
            })?;
            reject_cmd_metacharacters(permanent_ae)?;
            reject_cmd_metacharacters(&command_processor)?;
            Ok(format!(
                "\"{}\" /d /s /c \"\"{}\" start\"",
                command_processor.display(),
                permanent_ae.display()
            ))
        }
    }
}

fn reject_cmd_metacharacters(path: &Path) -> Result<()> {
    let path = path.to_str().ok_or_else(|| {
        CliError::Installation("the permanent ae script path is not valid Unicode".into())
    })?;
    if path.chars().any(|character| {
        matches!(
            character,
            '%' | '!' | '^' | '&' | '|' | '<' | '>' | '(' | ')' | '"' | '\r' | '\n'
        )
    }) {
        return Err(CliError::Installation(
            "the permanent ae script path contains characters unsafe for Windows cmd.exe".into(),
        ));
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn trusted_windows_command_processor() -> Result<PathBuf> {
    let system_root = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| {
            CliError::Installation(
                "Windows SystemRoot is unavailable; cannot safely schedule the permanent ae script"
                    .into(),
            )
        })?;
    let command_processor = system_root.join("System32").join("cmd.exe");
    if !command_processor.is_file() {
        return Err(CliError::Installation(format!(
            "trusted Windows command processor is unavailable at {}; run `ae doctor --fix`",
            command_processor.display()
        )));
    }
    Ok(command_processor)
}

#[cfg(not(target_os = "windows"))]
fn trusted_windows_command_processor() -> Result<PathBuf> {
    Err(unsupported())
}

#[cfg(not(target_os = "windows"))]
fn unsupported() -> CliError {
    CliError::Unsupported(
        "Forge autostart uses Windows Task Scheduler or a systemd user service and is unavailable on this platform"
            .into(),
    )
}

pub(in crate::commands) fn scheduled_task_create_args(action: &str) -> Vec<String> {
    vec![
        "/Create".into(),
        "/TN".into(),
        AUTOSTART_TASK_NAME.into(),
        "/TR".into(),
        action.into(),
        "/SC".into(),
        "ONLOGON".into(),
        "/RL".into(),
        "LIMITED".into(),
        "/F".into(),
    ]
}

#[cfg(target_os = "windows")]
fn create_autostart_task(action: &str) -> Result<()> {
    let status = hidden_schtasks(
        &scheduled_task_create_args(action),
        "create Forge autostart task",
    )?;
    if status.success() {
        Ok(())
    } else {
        Err(CliError::Control(format!(
            "could not create current-user Forge autostart task ({status})"
        )))
    }
}

#[cfg(not(target_os = "windows"))]
fn create_autostart_task(_: &str) -> Result<()> {
    Err(unsupported())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::commands) enum ScheduledTaskDeletion {
    AlreadyAbsent,
    Deleted,
    Failed,
}

/// Converts scheduler exit outcomes into idempotent removal semantics without
/// running a real task operation in unit tests.
pub(in crate::commands) fn scheduled_task_deletion(
    task_exists: bool,
    delete_succeeded: bool,
) -> ScheduledTaskDeletion {
    match (task_exists, delete_succeeded) {
        (false, _) => ScheduledTaskDeletion::AlreadyAbsent,
        (true, true) => ScheduledTaskDeletion::Deleted,
        (true, false) => ScheduledTaskDeletion::Failed,
    }
}

#[cfg(target_os = "windows")]
pub(super) fn disable() -> Result<()> {
    let query_status = hidden_schtasks(
        &["/Query".into(), "/TN".into(), AUTOSTART_TASK_NAME.into()],
        "inspect Forge autostart task before removal",
    )?;
    if matches!(
        scheduled_task_deletion(query_status.success(), true),
        ScheduledTaskDeletion::AlreadyAbsent
    ) {
        return Ok(());
    }
    let delete_status = hidden_schtasks(
        &[
            "/Delete".into(),
            "/TN".into(),
            AUTOSTART_TASK_NAME.into(),
            "/F".into(),
        ],
        "remove Forge autostart task",
    )?;
    match scheduled_task_deletion(true, delete_status.success()) {
        ScheduledTaskDeletion::Deleted => Ok(()),
        ScheduledTaskDeletion::AlreadyAbsent => unreachable!("task existence was checked first"),
        ScheduledTaskDeletion::Failed => Err(CliError::Control(format!(
            "could not remove current-user Forge autostart task ({delete_status})"
        ))),
    }
}

#[cfg(not(target_os = "windows"))]
pub(super) fn disable() -> Result<()> {
    Err(unsupported())
}

#[cfg(target_os = "windows")]
pub(super) fn enabled() -> Result<bool> {
    let status = hidden_schtasks(
        &["/Query".into(), "/TN".into(), AUTOSTART_TASK_NAME.into()],
        "inspect Forge autostart task",
    )?;
    Ok(status.success())
}

#[cfg(not(target_os = "windows"))]
pub(super) fn enabled() -> Result<bool> {
    Err(unsupported())
}

#[cfg(target_os = "windows")]
fn hidden_schtasks(
    arguments: &[String],
    context: &'static str,
) -> Result<std::process::ExitStatus> {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    Command::new("schtasks.exe")
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .map_err(io(context))
}
