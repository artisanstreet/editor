//! Running helper programs (systemctl, reg.exe, PowerShell, wsl.exe)
//! without console windows flashing up on Windows.

use anyhow::{Context, Result, bail};
use std::path::Path;
use std::process::{Command, Stdio};

/// Runs `program` and fails with its stderr when it fails.
pub fn run(program: &str, args: &[&str]) -> Result<()> {
    output(program, args).map(drop)
}

/// Runs `program` and returns its stdout. System tools run without
/// `LD_LIBRARY_PATH`: a Nix shell's libraries in it break the system's glibc
/// programs, systemctl among them.
pub fn output(program: &str, args: &[&str]) -> Result<String> {
    let output = quiet(Command::new(program).args(args))
        .env_remove("LD_LIBRARY_PATH")
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("run {program}"))?;
    if !output.status.success() {
        bail!(
            "{program} {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Runs a PowerShell script and fails with its error output.
pub fn powershell(script: &str) -> Result<String> {
    let output = powershell_command(script)
        .output()
        .context("run PowerShell")?;
    if !output.status.success() {
        bail!(
            "PowerShell failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub fn powershell_command(script: &str) -> Command {
    let mut command = Command::new("powershell.exe");
    command
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script,
        ])
        .stdin(Stdio::null());
    quiet(&mut command);
    command
}

/// A PowerShell single-quoted string.
pub fn ps_quote(value: &Path) -> String {
    ps_string(&value.display().to_string())
}

pub fn ps_string(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// No console window flashes up for helper processes on Windows.
#[allow(
    clippy::missing_const_for_fn,
    reason = "const only off Windows, where the body sets no flags"
)]
pub fn quiet(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// Starts `command` detached from this terminal: its own process group,
/// no console window on Windows.
pub fn spawn_detached(command: &mut Command) -> Result<u32> {
    command.stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let child = command
        .spawn()
        .with_context(|| format!("start {}", command.get_program().display()))?;
    Ok(child.id())
}

/// Removes a file, a symlink or a folder; absent is fine.
pub fn remove(path: &Path) -> Result<()> {
    let Ok(metadata) = path.symlink_metadata() else {
        return Ok(());
    };
    if metadata.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
    .with_context(|| format!("remove {}", path.display()))
}

/// Writes a file, creating its folder.
pub fn write(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    std::fs::write(path, contents).with_context(|| format!("write {}", path.display()))
}
