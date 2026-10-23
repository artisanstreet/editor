//! What an install sets up around an app, per OS: `ae` on PATH, the
//! Editor's launcher entry and `artisan://` handler, the Settings → Apps
//! entries on Windows, and closing running Editors before replacing them.

use crate::sys::{self, ps_quote, ps_string};
use anyhow::{Context, Result};
use artisan_paths::App;
use std::path::{Path, PathBuf};

const DESKTOP_FILE: &str = "artisan-editor.desktop";
/// The `.desktop` entry's `Icon=artisan-editor`.
const EDITOR_ICON: &str = include_str!("../../editor/assets/artisan-editor.svg");
const SHORTCUT: &str = "Artisan Editor.lnk";

/// Puts `ae` on PATH: its folder on the user `Path` on Windows,
/// `~/.local/bin/ae` elsewhere. Returns a hint when the shell won't see it.
pub fn add_to_path() -> Result<Option<String>> {
    if cfg!(windows) {
        let dir = ps_quote(&App::Ae.dir()?);
        // Keeps the value's REG_EXPAND_SZ type and other entries' %VARS%;
        // setting a throwaway variable broadcasts WM_SETTINGCHANGE so new
        // terminals see the change.
        sys::powershell(&format!(
            "$dir = {dir}; $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true); \
             $old = [string]$key.GetValue('Path', '', 'DoNotExpandEnvironmentNames'); \
             $parts = @($old -split ';' | Where-Object {{ $_ -and $_ -ne $dir }}); \
             $key.SetValue('Path', ((@($dir) + $parts) -join ';'), 'ExpandString'); \
             [Environment]::SetEnvironmentVariable('ArtisanPathRefresh', '1', 'User'); \
             [Environment]::SetEnvironmentVariable('ArtisanPathRefresh', $null, 'User')"
        ))?;
        Ok(None)
    } else {
        // A file of someone else's here was confirmed away (`foreign_ae`).
        let link = unix_link()?;
        if let Some(parent) = link.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
        }
        std::fs::remove_file(&link).ok();
        #[cfg(unix)]
        std::os::unix::fs::symlink(App::Ae.binary()?, &link)
            .with_context(|| format!("link {}", link.display()))?;
        let dir = link.parent().unwrap_or(&link);
        Ok((!on_path(dir)).then(|| {
            format!(
                "Add {} to your PATH to run `ae` from a terminal.",
                dir.display()
            )
        }))
    }
}

/// A `~/.local/bin/ae` that isn't Artisan's link, which installing replaces.
pub fn foreign_ae() -> Option<PathBuf> {
    if cfg!(windows) {
        return None;
    }
    let link = unix_link().ok()?;
    link.symlink_metadata()
        .is_ok_and(|metadata| !metadata.is_symlink())
        .then_some(link)
}

pub fn remove_from_path() -> Result<()> {
    if cfg!(windows) {
        remove_path_entry(&App::Ae.dir()?)
    } else {
        let link = unix_link()?;
        if std::fs::read_link(&link)
            .is_ok_and(|target| target == App::Ae.binary().unwrap_or_default())
        {
            sys::remove(&link)?;
        }
        Ok(())
    }
}

/// Drops one folder from the user `Path` (Windows).
pub fn remove_path_entry(dir: &Path) -> Result<()> {
    let dir = ps_quote(dir);
    sys::powershell(&format!(
        "$dir = {dir}; $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true); \
         $old = [string]$key.GetValue('Path', '', 'DoNotExpandEnvironmentNames'); \
         $new = (@($old -split ';' | Where-Object {{ $_ -and $_.TrimEnd('\\') -ne $dir.TrimEnd('\\') }})) -join ';'; \
         if ($new -ne $old) {{ $key.SetValue('Path', $new, 'ExpandString') }}"
    ))
    .map(drop)
}

/// Whether a new terminal finds this `ae`.
pub fn path_ok() -> bool {
    if cfg!(windows) {
        let Ok(dir) = App::Ae.dir() else {
            return false;
        };
        sys::output(
            "powershell.exe",
            &[
                "-NoProfile",
                "-Command",
                "[Environment]::GetEnvironmentVariable('Path','User')",
            ],
        )
        .is_ok_and(|path| path.split(';').any(|entry| Path::new(entry.trim()) == dir))
    } else {
        unix_link().is_ok_and(|link| {
            std::fs::read_link(&link)
                .is_ok_and(|target| target == App::Ae.binary().unwrap_or_default())
                && link.parent().is_some_and(on_path)
        })
    }
}

pub fn unix_link() -> Result<PathBuf> {
    Ok(artisan_paths::home()?.join(".local/bin/ae"))
}

fn on_path(dir: &Path) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|entry| entry == dir))
}

/// The Start menu entry and `artisan://` handler (Windows), or the
/// `.desktop` entry (Linux), pointing at the Editor's fixed path.
pub fn integrate_editor() -> Result<()> {
    let editor = App::Editor.binary()?;
    if cfg!(windows) {
        let link = start_menu()?.join(SHORTCUT);
        let command = format!("\"{}\" \"%1\"", editor.display());
        sys::powershell(&format!(
            "$shell = New-Object -ComObject WScript.Shell; $s = $shell.CreateShortcut({link}); \
             $s.TargetPath = {target}; $s.WorkingDirectory = {folder}; $s.IconLocation = {target} + ',0'; $s.Save(); \
             $key = 'HKCU:\\Software\\Classes\\artisan'; \
             New-Item -Path \"$key\\shell\\open\\command\" -Force | Out-Null; \
             Set-ItemProperty -Path $key -Name '(default)' -Value 'URL:Artisan Protocol'; \
             Set-ItemProperty -Path $key -Name 'URL Protocol' -Value ''; \
             Set-ItemProperty -Path \"$key\\shell\\open\\command\" -Name '(default)' -Value {command}",
            link = ps_quote(&link),
            target = ps_quote(&editor),
            folder = ps_quote(editor.parent().unwrap_or(&editor)),
            command = ps_string(&command),
        ))
        .map(drop)
    } else if cfg!(target_os = "linux") {
        let entry = format!(
            "[Desktop Entry]\nType=Application\nName=Artisan Editor\nExec=\"{}\" %u\nIcon=artisan-editor\nTerminal=false\n\
             Categories=Development;TextEditor;\nMimeType=x-scheme-handler/artisan;\nStartupWMClass=artisan-editor\n",
            editor.display()
        );
        sys::write(&desktop_file()?, &entry)?;
        sys::write(&icon_file()?, EDITOR_ICON)?;
        if let Err(error) = sys::run(
            "xdg-mime",
            &["default", DESKTOP_FILE, "x-scheme-handler/artisan"],
        ) {
            tracing::debug!("couldn't register artisan:// links: {error:#}");
        }
        Ok(())
    } else {
        Ok(())
    }
}

pub fn remove_editor_integration() -> Result<()> {
    if cfg!(windows) {
        sys::remove(&start_menu()?.join(SHORTCUT))?;
        if sys::output("reg.exe", &["query", r"HKCU\Software\Classes\artisan"]).is_ok() {
            sys::run(
                "reg.exe",
                &["delete", r"HKCU\Software\Classes\artisan", "/f"],
            )?;
        }
        Ok(())
    } else if cfg!(target_os = "linux") {
        sys::remove(&icon_file()?)?;
        sys::remove(&desktop_file()?)
    } else {
        Ok(())
    }
}

pub fn start_menu() -> Result<PathBuf> {
    let app_data = PathBuf::from(std::env::var_os("APPDATA").context("APPDATA is not set")?);
    Ok(app_data.join(r"Microsoft\Windows\Start Menu\Programs"))
}

fn desktop_file() -> Result<PathBuf> {
    Ok(artisan_paths::xdg_data()?
        .join("applications")
        .join(DESKTOP_FILE))
}

fn icon_file() -> Result<PathBuf> {
    Ok(artisan_paths::xdg_data()?
        .join("icons/hicolor/scalable/apps")
        .join("artisan-editor.svg"))
}

fn uninstall_key(app: App) -> String {
    format!(
        r"HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\ArtisanStreet.{}",
        app.id()
    )
}

/// The app's entry in Settings → Apps (Windows); uninstalling there runs
/// `ae uninstall <app> --yes`.
pub fn register_uninstall(app: App, version: &str) -> Result<()> {
    if !cfg!(windows) {
        return Ok(());
    }
    let name = match app {
        App::Ae => "Artisan command line (ae)",
        App::Forge => "Artisan Forge",
        App::Editor => "Artisan Editor",
    };
    let ae = App::Ae.binary()?;
    let uninstall = format!("\"{}\" uninstall {} --yes", ae.display(), app.id());
    sys::powershell(&format!(
        "$key = {key}; New-Item -Path $key -Force | Out-Null; \
         Set-ItemProperty -Path $key -Name DisplayName -Value {name}; \
         Set-ItemProperty -Path $key -Name DisplayVersion -Value {version}; \
         Set-ItemProperty -Path $key -Name Publisher -Value 'Artisan Street'; \
         Set-ItemProperty -Path $key -Name InstallLocation -Value {location}; \
         Set-ItemProperty -Path $key -Name DisplayIcon -Value {icon}; \
         Set-ItemProperty -Path $key -Name UninstallString -Value {uninstall}; \
         Set-ItemProperty -Path $key -Name QuietUninstallString -Value {uninstall}; \
         Set-ItemProperty -Path $key -Name NoModify -Value 1 -Type DWord; \
         Set-ItemProperty -Path $key -Name NoRepair -Value 1 -Type DWord",
        key = ps_string(&uninstall_key(app)),
        name = ps_string(name),
        version = ps_string(version),
        location = ps_quote(&app.dir()?),
        icon = ps_quote(&app.binary()?),
        uninstall = ps_string(&uninstall),
    ))
    .map(drop)
}

pub fn unregister_uninstall(app: App) -> Result<()> {
    if !cfg!(windows) {
        return Ok(());
    }
    sys::powershell(&format!(
        "Remove-Item -Path {} -Recurse -Force -ErrorAction SilentlyContinue",
        ps_string(&uninstall_key(app))
    ))
    .map(drop)
}

/// Starts the installed Editor, detached and outside this process's job
/// (Explorer starts it on Windows, so it outlives a WSL terminal).
pub fn launch_editor() -> Result<()> {
    let editor = App::Editor.binary()?;
    if cfg!(windows) {
        sys::spawn_detached(std::process::Command::new("explorer.exe").arg(&editor))?;
    } else {
        sys::spawn_detached(
            std::process::Command::new(&editor)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null()),
        )?;
    }
    Ok(())
}

/// Closes running processes started from `executables` (case-insensitive
/// on Windows), so their files can be replaced. Forge holds all data, so a
/// forced stop after 10 seconds loses nothing.
pub fn close_processes(executables: &[PathBuf]) -> Result<usize> {
    if cfg!(windows) {
        let list = executables
            .iter()
            .map(|path| ps_quote(path))
            .collect::<Vec<_>>()
            .join(",");
        let closed = sys::powershell(&format!(
            "$targets = @({list}); \
             $p = @(Get-CimInstance Win32_Process | Where-Object {{ $x = $_.ExecutablePath; $x -and ($targets | Where-Object {{ $_ -ieq $x }}) }}); \
             foreach ($x in $p) {{ taskkill.exe /PID $x.ProcessId | Out-Null }}; \
             foreach ($x in $p) {{ Wait-Process -Id $x.ProcessId -Timeout 10 -ErrorAction SilentlyContinue }}; \
             @($p | Where-Object {{ Get-Process -Id $_.ProcessId -ErrorAction Ignore }}) | ForEach-Object {{ Stop-Process -Id $_.ProcessId -Force }}; \
             $p.Count"
        ))?;
        Ok(closed.trim().parse().unwrap_or(0))
    } else {
        let mut system = sysinfo::System::new();
        system.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::All,
            sysinfo::ProcessRefreshKind::new().with_exe(sysinfo::UpdateKind::Always),
        );
        let running: Vec<_> = system
            .processes()
            .iter()
            .filter_map(|(pid, process)| {
                let exe = process.exe()?.to_string_lossy().into_owned();
                let exe = exe.strip_suffix(" (deleted)").unwrap_or(&exe);
                executables
                    .iter()
                    .any(|path| Path::new(exe) == path)
                    .then_some((*pid, process.start_time()))
            })
            .collect();
        for (pid, _) in &running {
            sys::run("kill", &["-TERM", &pid.to_string()]).ok();
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        for (pid, started) in &running {
            loop {
                let mut current = sysinfo::System::new();
                current.refresh_processes_specifics(
                    sysinfo::ProcessesToUpdate::Some(&[*pid]),
                    sysinfo::ProcessRefreshKind::new(),
                );
                let alive = current
                    .process(*pid)
                    .is_some_and(|process| process.start_time() == *started);
                if !alive {
                    break;
                }
                if std::time::Instant::now() >= deadline {
                    sys::run("kill", &["-KILL", &pid.to_string()]).ok();
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
        Ok(running.len())
    }
}

/// Whether a desktop is here to show the Editor.
pub fn has_desktop() -> bool {
    cfg!(any(windows, target_os = "macos"))
        || std::env::var_os("DISPLAY").is_some()
        || std::env::var_os("WAYLAND_DISPLAY").is_some()
}

/// Inside WSL (Linux with Windows interop).
pub fn in_wsl() -> bool {
    cfg!(target_os = "linux") && std::env::var_os("WSL_DISTRO_NAME").is_some()
}
