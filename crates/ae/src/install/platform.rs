//! PATH, shortcuts, the `artisan://` handler and stopping old Editors, per
//! OS. Every change goes through [`Sys`], so `--dry-run` only prints them.

use super::layout::{EDITOR, Root};
use super::sys::{Sys, output, ps_quote};
use crate::paths;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

const DESKTOP_FILE: &str = "artisan-editor.desktop";
const SHORTCUT: &str = "Artisan Editor.lnk";

/// Puts the stable `ae` on PATH: `~/.local/bin/ae` on Unix, the user `Path`
/// on Windows.
pub fn add_to_path(sys: Sys, root: &Root) -> Result<()> {
    if cfg!(windows) {
        let bin = ps_quote(&root.bin());
        // Keeps the value's REG_EXPAND_SZ type and other entries' %VARS%;
        // setting a throwaway variable broadcasts WM_SETTINGCHANGE so new
        // terminals see the change.
        sys.powershell(&format!(
            "$bin = {bin}; $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true); \
             $old = [string]$key.GetValue('Path', '', 'DoNotExpandEnvironmentNames'); \
             $parts = @($old -split ';' | Where-Object {{ $_ -and $_ -ne $bin }}); \
             $key.SetValue('Path', ((@($bin) + $parts) -join ';'), 'ExpandString'); \
             [Environment]::SetEnvironmentVariable('ArtisanPathRefresh', '1', 'User'); \
             [Environment]::SetEnvironmentVariable('ArtisanPathRefresh', $null, 'User')"
        ))
    } else {
        let link = unix_link()?;
        if link
            .symlink_metadata()
            .is_ok_and(|metadata| !metadata.is_symlink())
        {
            anyhow::bail!(
                "{} exists and is not Artisan's; remove it and run `ae doctor --fix`",
                link.display()
            );
        }
        #[cfg(unix)]
        sys.symlink(&root.ae(), &link)?;
        if !on_path(link.parent().unwrap_or(&link)) {
            println!(
                "Add {} to your PATH to run `ae` from a terminal.",
                link.parent().unwrap_or(&link).display()
            );
        }
        Ok(())
    }
}

pub fn remove_from_path(sys: Sys, root: &Root) -> Result<()> {
    if cfg!(windows) {
        let bin = ps_quote(&root.bin());
        sys.powershell(&format!(
            "$bin = {bin}; $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true); \
             $old = [string]$key.GetValue('Path', '', 'DoNotExpandEnvironmentNames'); \
             $key.SetValue('Path', ((@($old -split ';' | Where-Object {{ $_ -and $_ -ne $bin }})) -join ';'), 'ExpandString')"
        ))
    } else {
        let link = unix_link()?;
        if std::fs::read_link(&link).is_ok_and(|target| target == root.ae()) {
            sys.remove(&link)?;
        }
        Ok(())
    }
}

/// Whether running `ae` from a new terminal reaches this install.
pub fn path_ok(root: &Root) -> bool {
    if cfg!(windows) {
        output(
            "powershell.exe",
            &[
                "-NoProfile",
                "-Command",
                "[Environment]::GetEnvironmentVariable('Path','User')",
            ],
        )
        .is_ok_and(|path| {
            path.split(';')
                .any(|entry| Path::new(entry.trim()) == root.bin())
        })
    } else {
        unix_link()
            .is_ok_and(|link| std::fs::read_link(link).is_ok_and(|target| target == root.ae()))
    }
}

fn unix_link() -> Result<PathBuf> {
    Ok(paths::home()?.join(".local").join("bin").join("ae"))
}

fn on_path(dir: &Path) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|entry| entry == dir))
}

/// Start menu and desktop shortcuts and the `artisan://` handler, pointing
/// straight at this version's Editor (rewritten on every install, so the
/// icon never points at a deleted version).
pub fn integrate_editor(sys: Sys, editor: &Path) -> Result<()> {
    if cfg!(windows) {
        let target = ps_quote(editor);
        let folder = ps_quote(editor.parent().unwrap_or(editor));
        let links = shortcut_paths()?;
        let script = links
            .iter()
            .map(|link| {
                format!(
                    "$s = $shell.CreateShortcut({}); $s.TargetPath = {target}; $s.WorkingDirectory = {folder}; \
                     $s.IconLocation = {target} + ',0'; $s.Save()",
                    ps_quote(link)
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        sys.powershell(&format!(
            "$shell = New-Object -ComObject WScript.Shell; {script}"
        ))?;
        let command = format!("\"{}\" \"%1\"", editor.display());
        sys.run(
            "reg.exe",
            &[
                "add",
                r"HKCU\Software\Classes\artisan",
                "/ve",
                "/d",
                "URL:Artisan Protocol",
                "/f",
            ],
        )?;
        sys.run(
            "reg.exe",
            &[
                "add",
                r"HKCU\Software\Classes\artisan",
                "/v",
                "URL Protocol",
                "/d",
                "",
                "/f",
            ],
        )?;
        sys.run(
            "reg.exe",
            &[
                "add",
                r"HKCU\Software\Classes\artisan\shell\open\command",
                "/ve",
                "/d",
                &command,
                "/f",
            ],
        )
    } else if cfg!(target_os = "linux") {
        let entry = format!(
            "[Desktop Entry]\nType=Application\nName=Artisan Editor\nExec=\"{}\" %u\nIcon={EDITOR}\nTerminal=false\n\
             Categories=Development;TextEditor;\nMimeType=x-scheme-handler/artisan;\nStartupWMClass={EDITOR}\n",
            editor.display()
        );
        sys.write(&desktop_file()?, &entry)?;
        if let Err(error) = sys.run(
            "xdg-mime",
            &["default", DESKTOP_FILE, "x-scheme-handler/artisan"],
        ) {
            println!("Couldn't register artisan:// links ({error:#}); the Editor still works.");
        }
        Ok(())
    } else {
        println!("Shortcuts and artisan:// links aren't set up on this OS; run `ae editor open`.");
        Ok(())
    }
}

pub fn remove_editor_integration(sys: Sys) -> Result<()> {
    if cfg!(windows) {
        for link in shortcut_paths()? {
            sys.remove(&link)?;
        }
        if output("reg.exe", &["query", r"HKCU\Software\Classes\artisan"]).is_ok() {
            sys.run(
                "reg.exe",
                &["delete", r"HKCU\Software\Classes\artisan", "/f"],
            )?;
        }
        Ok(())
    } else {
        sys.remove(&desktop_file()?)
    }
}

/// Whether the shortcuts are present (`ae doctor`).
pub fn editor_integrated() -> bool {
    if cfg!(windows) {
        shortcut_paths().is_ok_and(|links| links.iter().all(|link| link.exists()))
    } else if cfg!(target_os = "linux") {
        desktop_file().is_ok_and(|file| file.exists())
    } else {
        true
    }
}

fn shortcut_paths() -> Result<Vec<PathBuf>> {
    let app_data = PathBuf::from(std::env::var_os("APPDATA").context("APPDATA is not set")?);
    Ok(vec![
        app_data
            .join(r"Microsoft\Windows\Start Menu\Programs")
            .join(SHORTCUT),
        paths::home()?.join("Desktop").join(SHORTCUT),
    ])
}

fn desktop_file() -> Result<PathBuf> {
    Ok(paths::xdg_data()?.join("applications").join(DESKTOP_FILE))
}

/// Closes Editors running from this install, so files can be replaced and
/// the new Editor is the one that opens (the Editor is single-instance).
/// Forge holds all data, so a forced stop loses nothing.
pub fn retire_editors(sys: Sys, root: &Root) -> Result<()> {
    let versions = root.versions();
    if cfg!(windows) {
        let prefix = ps_quote(&versions.join(""));
        sys.powershell(&format!(
            "$p = @(Get-CimInstance Win32_Process | Where-Object {{ $_.ExecutablePath -and \
             $_.ExecutablePath.StartsWith({prefix}, 'OrdinalIgnoreCase') }}); \
             foreach ($x in $p) {{ taskkill.exe /PID $x.ProcessId | Out-Null }}; \
             foreach ($x in $p) {{ Wait-Process -Id $x.ProcessId -Timeout 10 -ErrorAction SilentlyContinue }}; \
             foreach ($x in $p) {{ Stop-Process -Id $x.ProcessId -Force -ErrorAction SilentlyContinue }}"
        ))
    } else {
        let Ok(processes) = std::fs::read_dir("/proc") else {
            return Ok(());
        };
        for process in processes.flatten() {
            let pid = process.file_name().to_string_lossy().into_owned();
            if pid.bytes().all(|byte| byte.is_ascii_digit())
                && std::fs::read_link(process.path().join("exe"))
                    .is_ok_and(|exe| exe.starts_with(&versions))
            {
                println!("Closing the running Editor (pid {pid}).");
                sys.run("kill", &["-TERM", &pid])?;
            }
        }
        Ok(())
    }
}

/// Runs `program` detached from this console (the Editor, Forge on
/// Windows/without systemd).
pub fn spawn_detached(command: &mut std::process::Command) -> Result<u32> {
    command.stdin(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW rather than DETACHED_PROCESS: Forge's own console
        // children (git, shells) would each open a window otherwise.
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

/// Lists WSL distributions (from Windows, or from inside WSL via interop).
pub fn wsl_distros() -> Result<Vec<String>> {
    let output = std::process::Command::new("wsl.exe")
        .args(["--list", "--quiet"])
        .stdin(std::process::Stdio::null())
        .output()
        .context("run wsl.exe")?;
    // wsl.exe writes UTF-16LE.
    let units: Vec<u16> = output
        .stdout
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect();
    Ok(String::from_utf16_lossy(&units)
        .lines()
        .map(|line| {
            line.trim_matches(|c: char| c.is_whitespace() || c == '\0')
                .to_owned()
        })
        .filter(|line| !line.is_empty())
        .collect())
}
