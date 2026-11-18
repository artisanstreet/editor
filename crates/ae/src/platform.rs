//! What an install sets up around an app, per OS: `ae` on PATH, the
//! Editor's launcher entry and `artisan://` handler, the Settings → Apps
//! entries on Windows, and closing running Editors before replacing them.

use crate::sys::{self, ps_quote, ps_string};
use anyhow::{Context, Result};
use artisan_paths::App;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const DESKTOP_FILE: &str = "artisan-editor.desktop";
const EDITOR_ICON: &str = include_str!("../../editor-assets/app/artisan-editor.svg");
const SHORTCUT: &str = "Artisan Editor.lnk";
const PROTOCOL_KEY: &str = r"HKCU:\Software\Classes\artisan";
const KEEP_OTHER_ENTRIES: &str = r"$_ -and $_.TrimEnd('\') -ne $dir.TrimEnd('\')";

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
             $parts = @($old -split ';' | Where-Object {{ {KEEP_OTHER_ENTRIES} }}); \
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
         $new = (@($old -split ';' | Where-Object {{ {KEEP_OTHER_ENTRIES} }})) -join ';'; \
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
        let command = protocol_command(&editor);
        sys::powershell(&format!(
            "$shell = New-Object -ComObject WScript.Shell; $s = $shell.CreateShortcut({link}); \
             $s.TargetPath = {target}; $s.WorkingDirectory = {folder}; $s.IconLocation = {target} + ',0'; $s.Save(); \
             $key = {protocol}; \
             New-Item -Path \"$key\\shell\\open\\command\" -Force | Out-Null; \
             Set-ItemProperty -Path $key -Name '(default)' -Value 'URL:Artisan Protocol'; \
             Set-ItemProperty -Path $key -Name 'URL Protocol' -Value ''; \
             Set-ItemProperty -Path \"$key\\shell\\open\\command\" -Name '(default)' -Value {command}",
            link = ps_quote(&link),
            target = ps_quote(&editor),
            folder = ps_quote(editor.parent().unwrap_or(&editor)),
            command = ps_string(&command),
            protocol = ps_string(PROTOCOL_KEY),
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
        let registered = sys::powershell(&format!(
            "$v = (Get-ItemProperty -LiteralPath {key} -ErrorAction SilentlyContinue).'(default)'; if ($v) {{ $v }}",
            key = ps_string(&format!(r"{PROTOCOL_KEY}\shell\open\command")),
        ))?;
        if registered.trim() == protocol_command(&App::Editor.binary()?) {
            sys::powershell(&format!(
                "Remove-Item -LiteralPath {} -Recurse -Force",
                ps_string(PROTOCOL_KEY)
            ))?;
        }
        Ok(())
    } else if cfg!(target_os = "linux") {
        sys::remove(&icon_file()?)?;
        sys::remove(&desktop_file()?)
    } else {
        Ok(())
    }
}

fn protocol_command(editor: &Path) -> String {
    format!("\"{}\" \"%1\"", editor.display())
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

const CLOSE_DEADLINE: Duration = Duration::from_secs(10);
const FORCE_DEADLINE: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Running {
    pub pid: u32,
    pub executable: PathBuf,
    pub started: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum StopError {
    #[error("process {pid} ({}) still runs after it was force-stopped", executable.display())]
    Survived { pid: u32, executable: PathBuf },
}

pub fn close_processes(executables: &[PathBuf]) -> Result<usize, StopError> {
    let running: Vec<Running> = processes(sysinfo::ProcessesToUpdate::All)
        .into_iter()
        .filter(|process| {
            executables
                .iter()
                .any(|path| same_executable(&process.executable, path))
        })
        .collect();
    stop(&running)?;
    Ok(running.len())
}

pub fn running(pid: u32) -> Option<Running> {
    processes(sysinfo::ProcessesToUpdate::Some(&[sysinfo::Pid::from_u32(
        pid,
    )]))
    .into_iter()
    .next()
}

pub fn stop(processes: &[Running]) -> Result<(), StopError> {
    for process in processes {
        request_close(process);
    }
    let started = Instant::now();
    let mut left: Vec<&Running> = processes.iter().collect();
    while !left.is_empty() && started.elapsed() < CLOSE_DEADLINE {
        std::thread::sleep(Duration::from_millis(100));
        left.retain(|process| alive(process));
    }
    for process in &left {
        force_close(process);
    }
    let forced = Instant::now();
    while !left.is_empty() && forced.elapsed() < FORCE_DEADLINE {
        std::thread::sleep(Duration::from_millis(50));
        left.retain(|process| alive(process));
    }
    match left.first() {
        Some(process) => Err(StopError::Survived {
            pid: process.pid,
            executable: process.executable.clone(),
        }),
        None => Ok(()),
    }
}

fn processes(which: sysinfo::ProcessesToUpdate<'_>) -> Vec<Running> {
    let mut system = sysinfo::System::new();
    system.refresh_processes_specifics(
        which,
        sysinfo::ProcessRefreshKind::new().with_exe(sysinfo::UpdateKind::Always),
    );
    system
        .processes()
        .iter()
        .filter(|(_, process)| {
            !matches!(
                process.status(),
                sysinfo::ProcessStatus::Zombie | sysinfo::ProcessStatus::Dead
            )
        })
        .filter_map(|(pid, process)| {
            let executable = process.exe()?.to_string_lossy();
            let executable = executable.strip_suffix(" (deleted)").unwrap_or(&executable);
            Some(Running {
                pid: pid.as_u32(),
                executable: PathBuf::from(executable),
                started: process.start_time(),
            })
        })
        .collect()
}

fn alive(process: &Running) -> bool {
    running(process.pid).is_some_and(|now| now.started == process.started)
}

pub fn same_executable(left: &Path, right: &Path) -> bool {
    if cfg!(windows) {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    } else {
        left == right
    }
}

fn request_close(process: &Running) {
    let pid = process.pid.to_string();
    let requested = if cfg!(windows) {
        sys::run("taskkill.exe", &["/PID", &pid])
    } else {
        sys::run("kill", &["-TERM", &pid])
    };
    if let Err(error) = requested {
        tracing::debug!(
            pid = process.pid,
            "asking the process to close failed: {error:#}"
        );
    }
}

fn force_close(process: &Running) {
    let pid = process.pid.to_string();
    let forced = if cfg!(windows) {
        sys::run("taskkill.exe", &["/PID", &pid, "/F"])
    } else {
        sys::run("kill", &["-KILL", &pid])
    };
    if let Err(error) = forced {
        tracing::debug!(
            pid = process.pid,
            "force-stopping the process failed: {error:#}"
        );
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
