//! Changes outside Artisan's install directory (PATH, shortcuts, the URL
//! handler, the service, other processes). With `--dry-run` they are
//! printed instead of made.

use anyhow::{Context, Result, bail};
use std::path::Path;
use std::process::{Command, Stdio};

#[derive(Clone, Copy)]
pub struct Sys {
    pub dry_run: bool,
}

impl Sys {
    pub fn run(self, program: &str, args: &[&str]) -> Result<()> {
        if self.dry_run {
            println!("would run: {program} {}", args.join(" "));
            return Ok(());
        }
        let status = quiet(Command::new(program).args(args))
            .stdin(Stdio::null())
            .status()
            .with_context(|| format!("run {program}"))?;
        if !status.success() {
            bail!("{program} {} failed ({status})", args.join(" "));
        }
        Ok(())
    }

    pub fn powershell(self, script: &str) -> Result<()> {
        if self.dry_run {
            println!("would run PowerShell: {script}");
            return Ok(());
        }
        let output = powershell(script).output().context("run PowerShell")?;
        if !output.status.success() {
            bail!(
                "PowerShell failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(())
    }

    pub fn write(self, path: &Path, contents: &str) -> Result<()> {
        if self.dry_run {
            println!("would write {}", path.display());
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
        }
        std::fs::write(path, contents).with_context(|| format!("write {}", path.display()))
    }

    /// Removes a file, a symlink or a directory; absent is fine.
    pub fn remove(self, path: &Path) -> Result<()> {
        let Ok(metadata) = path.symlink_metadata() else {
            return Ok(());
        };
        if self.dry_run {
            println!("would remove {}", path.display());
            return Ok(());
        }
        if metadata.is_dir() {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_file(path)
        }
        .with_context(|| format!("remove {}", path.display()))
    }

    #[cfg(unix)]
    pub fn symlink(self, target: &Path, link: &Path) -> Result<()> {
        if self.dry_run {
            println!("would link {} -> {}", link.display(), target.display());
            return Ok(());
        }
        if let Some(parent) = link.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
        }
        let _ = std::fs::remove_file(link);
        std::os::unix::fs::symlink(target, link).with_context(|| format!("link {}", link.display()))
    }
}

/// Runs a read-only query and returns its stdout.
pub fn output(program: &str, args: &[&str]) -> Result<String> {
    let output = quiet(Command::new(program).args(args))
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

pub fn powershell(script: &str) -> Command {
    let mut command = Command::new("powershell.exe");
    command.args([
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        script,
    ]);
    quiet(&mut command);
    command
}

/// A PowerShell single-quoted string.
pub fn ps_quote(value: &Path) -> String {
    format!("'{}'", value.display().to_string().replace('\'', "''"))
}

/// No console window flashes up for helper processes on Windows.
pub fn quiet(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}
