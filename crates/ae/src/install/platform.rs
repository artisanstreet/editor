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
        unix_link().is_ok_and(|link| {
            std::fs::read_link(&link).is_ok_and(|target| target == root.ae())
                && link.parent().is_some_and(on_path)
        })
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
        let query = "ConvertTo-Json -Compress -InputObject @(Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath } | ForEach-Object { [pscustomobject]@{ pid=$_.ProcessId; executable=$_.ExecutablePath; created=$_.CreationDate.ToString('o') } })";
        let output = super::sys::powershell(query)
            .output()
            .context("query running Editor identities")?;
        anyhow::ensure!(
            output.status.success(),
            "cannot inspect running Windows processes"
        );
        let processes: Vec<WindowsProcess> = serde_json::from_slice(&output.stdout)?;
        for process in processes
            .into_iter()
            .filter(|process| process.is_editor(&versions.to_string_lossy()))
        {
            if sys.dry_run {
                println!(
                    "would close Editor pid {} from {}",
                    process.pid, process.executable
                );
                continue;
            }
            let query = format!(
                "ConvertTo-Json -Compress -InputObject @(Get-CimInstance Win32_Process -Filter 'ProcessId = {}' | Where-Object {{ $_.ExecutablePath }} | ForEach-Object {{ [pscustomobject]@{{ pid=$_.ProcessId; executable=$_.ExecutablePath; created=$_.CreationDate.ToString('o') }} }})",
                process.pid
            );
            let output = super::sys::powershell(&query)
                .output()
                .context("recheck Editor identity")?;
            anyhow::ensure!(output.status.success(), "cannot recheck Editor identity");
            let current: Vec<WindowsProcess> = serde_json::from_slice(&output.stdout)?;
            if !current
                .first()
                .is_some_and(|current| process.same_identity(current))
            {
                continue;
            }
            let executable = ps_quote(Path::new(&process.executable));
            let created = format!("'{}'", process.created.replace('\'', "''"));
            sys.powershell(&format!(
                "$x = Get-CimInstance Win32_Process -Filter 'ProcessId = {pid}'; \
                 if ($x -and $x.ExecutablePath -ieq {executable} -and $x.CreationDate.ToString('o') -eq {created}) {{ \
                   taskkill.exe /PID {pid} | Out-Null; \
                   Wait-Process -Id {pid} -Timeout 10 -ErrorAction SilentlyContinue; \
                   $current = Get-CimInstance Win32_Process -Filter 'ProcessId = {pid}'; \
                   if ($current -and $current.ExecutablePath -ieq {executable} -and $current.CreationDate.ToString('o') -eq {created}) {{ Stop-Process -Id {pid} -Force -ErrorAction SilentlyContinue }} \
                 }}", pid=process.pid
            ))?;
        }
        Ok(())
    } else {
        let mut system = sysinfo::System::new();
        system.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::All,
            sysinfo::ProcessRefreshKind::new().with_exe(sysinfo::UpdateKind::Always),
        );
        let editors = system
            .processes()
            .iter()
            .filter_map(|(pid, process)| {
                let executable = process.exe()?;
                let filename = executable.file_name()?.to_string_lossy();
                (executable.starts_with(&versions)
                    && matches!(
                        filename.as_ref(),
                        "artisan-editor" | "artisan-editor (deleted)"
                    ))
                .then_some((*pid, process.start_time()))
            })
            .collect::<Vec<_>>();
        for (pid, started) in editors {
            println!("Closing the running Editor (pid {pid}).");
            sys.run("kill", &["-TERM", &pid.to_string()])?;
            if sys.dry_run {
                continue;
            }
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                let mut current = sysinfo::System::new();
                current.refresh_processes_specifics(
                    sysinfo::ProcessesToUpdate::Some(&[pid]),
                    sysinfo::ProcessRefreshKind::new().with_exe(sysinfo::UpdateKind::Always),
                );
                let alive = current
                    .process(pid)
                    .is_some_and(|process| process.start_time() == started);
                if !alive {
                    break;
                }
                anyhow::ensure!(
                    std::time::Instant::now() < deadline,
                    "Editor pid {pid} did not close within 10 seconds"
                );
                std::thread::sleep(std::time::Duration::from_millis(100));
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

#[derive(serde::Deserialize)]
struct WindowsProcess {
    pid: u32,
    executable: String,
    created: String,
}

impl WindowsProcess {
    fn is_editor(&self, versions: &str) -> bool {
        let executable = self.executable.replace('\\', "/").to_ascii_lowercase();
        let prefix = format!(
            "{}/",
            versions
                .replace('\\', "/")
                .trim_end_matches('/')
                .to_ascii_lowercase()
        );
        executable.starts_with(&prefix)
            && executable.rsplit('/').next() == Some("artisan-editor.exe")
    }

    fn same_identity(&self, current: &Self) -> bool {
        self.pid == current.pid
            && self.created == current.created
            && self.executable.eq_ignore_ascii_case(&current.executable)
    }
}

#[cfg(test)]
mod process_selection_tests {
    use super::*;
    fn process(executable: &str, created: &str) -> WindowsProcess {
        WindowsProcess {
            pid: 42,
            executable: executable.into(),
            created: created.into(),
        }
    }
    #[test]
    fn windows_retirement_selects_only_editor_role_in_this_install() {
        let root = r"C:\Artisan\versions";
        assert!(process(r"C:\ARTISAN\versions\v1\ARTISAN-EDITOR.EXE", "a").is_editor(root));
        assert!(!process(r"C:\Artisan\versions\v1\ae.exe", "a").is_editor(root));
        assert!(!process(r"C:\Artisan\versions-other\v1\artisan-editor.exe", "a").is_editor(root));
        assert!(!process(r"C:\Other\versions\v1\artisan-editor.exe", "a").is_editor(root));
    }
    #[test]
    fn recycled_windows_pid_fails_the_identity_recheck() {
        let original = process(r"C:\Artisan\versions\v1\artisan-editor.exe", "a");
        assert!(
            original.same_identity(&process(r"C:\ARTISAN\versions\v1\artisan-editor.exe", "a"))
        );
        assert!(
            !original.same_identity(&process(r"C:\Artisan\versions\v1\artisan-editor.exe", "b"))
        );
        assert!(!original.same_identity(&process(r"C:\Artisan\versions\v1\ae.exe", "a")));
    }
}
