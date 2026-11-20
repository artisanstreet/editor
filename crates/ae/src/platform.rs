use crate::paths::{self, LocateError};
use crate::sys::{self, RemoveError, RunError, SpawnError, WriteError, ps_quote, ps_string};
use artisan_paths::{App, EnvPathError};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const DESKTOP_FILE: &str = "artisan-editor.desktop";
const EDITOR_ICON: &str = include_str!("../../editor-assets/app/artisan-editor.svg");
const SHORTCUT: &str = "Artisan Editor.lnk";
const PROTOCOL_KEY: &str = r"HKCU:\Software\Classes\artisan";
const KEEP_OTHER_ENTRIES: &str = r"$_ -and $_.TrimEnd('\') -ne $dir.TrimEnd('\')";

#[derive(Debug, thiserror::Error)]
pub(crate) enum AddToPathError {
    #[error(transparent)]
    Locate(#[from] LocateError),
    #[error("add ae's folder to the user Path")]
    PowerShell(#[source] RunError),
    #[error("find the home folder")]
    Home(#[source] EnvPathError),
    #[error("create {}", path.display())]
    CreateDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("look at {}", path.display())]
    Inspect {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{} changed during the install and isn't Artisan's `ae`, so it was left alone", path.display())]
    Changed { path: PathBuf },
    #[error("link {}", path.display())]
    Link {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ExistingAeError {
    #[error(transparent)]
    Locate(#[from] LocateError),
    #[error("find the home folder")]
    Home(#[source] EnvPathError),
    #[error("look at {}", path.display())]
    Inspect {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum RemoveFromPathError {
    #[error(transparent)]
    Locate(#[from] LocateError),
    #[error("drop ae's folder from the user Path")]
    PowerShell(#[source] RunError),
    #[error("find the home folder")]
    Home(#[source] EnvPathError),
    #[error("look at {}", path.display())]
    Inspect {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Remove(#[from] RemoveError),
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum IntegrateEditorError {
    #[error(transparent)]
    Locate(#[from] LocateError),
    #[error("APPDATA is not set")]
    NoAppData,
    #[error("find the user data folder")]
    Data(#[source] EnvPathError),
    #[error("add the Editor's Start menu entry and artisan:// handler")]
    PowerShell(#[source] RunError),
    #[error(transparent)]
    Write(#[from] WriteError),
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum RemoveEditorIntegrationError {
    #[error(transparent)]
    Locate(#[from] LocateError),
    #[error("APPDATA is not set")]
    NoAppData,
    #[error("find the user data folder")]
    Data(#[source] EnvPathError),
    #[error("remove the Editor's artisan:// handler")]
    PowerShell(#[source] RunError),
    #[error(transparent)]
    Remove(#[from] RemoveError),
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum RegisterUninstallError {
    #[error(transparent)]
    Locate(#[from] LocateError),
    #[error("add {} to Settings → Apps", app.display_name())]
    PowerShell {
        app: App,
        #[source]
        source: RunError,
    },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum LaunchEditorError {
    #[error(transparent)]
    Locate(#[from] LocateError),
    #[error(transparent)]
    Spawn(#[from] SpawnError),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Held {
    Nothing,
    Link(PathBuf),
    File { sha256: String },
    Other,
}

#[derive(Debug)]
pub(crate) struct Replaceable(Held);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Found(Held);

impl Found {
    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        let held = match value.split_once(':') {
            Some(("link", target)) => Held::Link(PathBuf::from(target)),
            Some(("file", sha256)) => Held::File {
                sha256: sha256.to_owned(),
            },
            None if value == "nothing" => Held::Nothing,
            None if value == "other" => Held::Other,
            Some(_) | None => return Err(format!("{value:?} doesn't describe a file")),
        };
        Ok(Self(held))
    }

    pub(crate) fn argument(&self) -> String {
        match &self.0 {
            Held::Nothing => "nothing".to_owned(),
            Held::Link(target) => format!("link:{}", target.to_string_lossy()),
            Held::File { sha256 } => format!("file:{sha256}"),
            Held::Other => "other".to_owned(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Consent {
    pub(crate) path: PathBuf,
    pub(crate) found: Found,
}

#[derive(Debug)]
pub(crate) enum ExistingAe {
    Artisans(Replaceable),
    Foreign {
        path: PathBuf,
        replaceable: Replaceable,
    },
}

fn held(link: &Path) -> std::io::Result<Held> {
    use sha2::Digest as _;
    match link.symlink_metadata() {
        Ok(metadata) if metadata.is_symlink() => Ok(Held::Link(std::fs::read_link(link)?)),
        Ok(metadata) if metadata.is_file() => Ok(Held::File {
            sha256: hex::encode(sha2::Sha256::digest(std::fs::read(link)?)),
        }),
        Ok(_) => Ok(Held::Other),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Held::Nothing),
        Err(error) => Err(error),
    }
}

fn artisans(held: &Held, binary: &Path) -> bool {
    match held {
        Held::Nothing => true,
        Held::Link(target) => target == binary,
        Held::File { .. } | Held::Other => false,
    }
}

impl Replaceable {
    pub(crate) fn consent(&self, path: &Path) -> Consent {
        Consent {
            path: path.to_owned(),
            found: Found(self.0.clone()),
        }
    }
}

pub(crate) fn existing_ae() -> Result<ExistingAe, ExistingAeError> {
    if cfg!(windows) {
        return Ok(ExistingAe::Artisans(Replaceable(Held::Nothing)));
    }
    let link = unix_link().map_err(ExistingAeError::Home)?;
    let binary = paths::binary(App::Ae)?;
    let found = held(&link).map_err(|source| ExistingAeError::Inspect {
        path: link.clone(),
        source,
    })?;
    Ok(if artisans(&found, &binary) {
        ExistingAe::Artisans(Replaceable(found))
    } else {
        ExistingAe::Foreign {
            path: link,
            replaceable: Replaceable(found),
        }
    })
}

pub(crate) fn add_to_path(replaceable: &Replaceable) -> Result<Option<String>, AddToPathError> {
    if cfg!(windows) {
        let dir = ps_quote(&paths::dir(App::Ae)?);
        sys::powershell(&format!(
            "$dir = {dir}; $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true); \
             $old = [string]$key.GetValue('Path', '', 'DoNotExpandEnvironmentNames'); \
             $parts = @($old -split ';' | Where-Object {{ {KEEP_OTHER_ENTRIES} }}); \
             $key.SetValue('Path', ((@($dir) + $parts) -join ';'), 'ExpandString'); \
             [Environment]::SetEnvironmentVariable('ArtisanPathRefresh', '1', 'User'); \
             [Environment]::SetEnvironmentVariable('ArtisanPathRefresh', $null, 'User')"
        ))
        .map_err(AddToPathError::PowerShell)?;
        Ok(None)
    } else {
        let link = unix_link().map_err(AddToPathError::Home)?;
        let binary = paths::binary(App::Ae)?;
        let dir = link
            .parent()
            .expect("invariant: ~/.local/bin/ae has a folder");
        std::fs::create_dir_all(dir).map_err(|source| AddToPathError::CreateDir {
            path: dir.to_owned(),
            source,
        })?;
        let now = held(&link).map_err(|source| AddToPathError::Inspect {
            path: link.clone(),
            source,
        })?;
        if now != replaceable.0 && !artisans(&now, &binary) {
            return Err(AddToPathError::Changed { path: link });
        }
        link_over(&binary, &link)?;
        Ok((!on_path(dir)).then(|| {
            format!(
                "Add {} to your PATH to run `ae` from a terminal.",
                dir.display()
            )
        }))
    }
}

fn link_over(binary: &Path, link: &Path) -> Result<(), AddToPathError> {
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    #[cfg(windows)]
    use std::os::windows::fs::symlink_file as symlink;
    let linking = |source| AddToPathError::Link {
        path: link.to_owned(),
        source,
    };
    let dir = link
        .parent()
        .expect("invariant: ~/.local/bin/ae has a folder");
    tempfile::Builder::new()
        .prefix(".ae-")
        .make_in(dir, |incoming| symlink(binary, incoming))
        .map_err(linking)?
        .persist(link)
        .map_err(|failed| linking(failed.error))
}

pub(crate) fn remove_from_path() -> Result<(), RemoveFromPathError> {
    if cfg!(windows) {
        remove_path_entry(&paths::dir(App::Ae)?).map_err(RemoveFromPathError::PowerShell)
    } else {
        let link = unix_link().map_err(RemoveFromPathError::Home)?;
        let binary = paths::binary(App::Ae)?;
        let found = held(&link).map_err(|source| RemoveFromPathError::Inspect {
            path: link.clone(),
            source,
        })?;
        if matches!(&found, Held::Link(target) if *target == binary) {
            sys::remove(&link)?;
        }
        Ok(())
    }
}

pub(crate) fn remove_path_entry(dir: &Path) -> Result<(), RunError> {
    let dir = ps_quote(dir);
    sys::powershell(&format!(
        "$dir = {dir}; $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true); \
         $old = [string]$key.GetValue('Path', '', 'DoNotExpandEnvironmentNames'); \
         $new = (@($old -split ';' | Where-Object {{ {KEEP_OTHER_ENTRIES} }})) -join ';'; \
         if ($new -ne $old) {{ $key.SetValue('Path', $new, 'ExpandString') }}"
    ))
    .map(drop)
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum PathCheckError {
    #[error("find ae's folder")]
    Locate(#[source] artisan_paths::RootError),
    #[error("read the user Path from the registry")]
    Registry(#[source] RunError),
    #[error("find the home folder")]
    Home(#[source] EnvPathError),
    #[error("read the link {}", path.display())]
    Link {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub(crate) fn path_ok() -> Result<bool, PathCheckError> {
    if cfg!(windows) {
        let dir = App::Ae.dir().map_err(PathCheckError::Locate)?;
        let path = sys::output(
            "powershell.exe",
            &[
                "-NoProfile",
                "-Command",
                "[Environment]::GetEnvironmentVariable('Path','User')",
            ],
        )
        .map_err(PathCheckError::Registry)?;
        Ok(path.split(';').any(|entry| Path::new(entry.trim()) == dir))
    } else {
        let link = unix_link().map_err(PathCheckError::Home)?;
        let ae = App::Ae.binary().map_err(PathCheckError::Locate)?;
        let target = match std::fs::read_link(&link) {
            Ok(target) => target,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::InvalidInput
                ) =>
            {
                return Ok(false);
            }
            Err(source) => return Err(PathCheckError::Link { path: link, source }),
        };
        Ok(target == ae && link.parent().is_some_and(on_path))
    }
}

pub(crate) fn unix_link() -> Result<PathBuf, EnvPathError> {
    Ok(artisan_paths::home()?.join(".local/bin/ae"))
}

fn on_path(dir: &Path) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|entry| entry == dir))
}

pub(crate) fn integrate_editor() -> Result<(), IntegrateEditorError> {
    let editor = paths::binary(App::Editor)?;
    if cfg!(windows) {
        let link = start_menu()
            .ok_or(IntegrateEditorError::NoAppData)?
            .join(SHORTCUT);
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
        .map_err(IntegrateEditorError::PowerShell)
    } else if cfg!(target_os = "linux") {
        let entry = format!(
            "[Desktop Entry]\nType=Application\nName=Artisan Editor\nExec=\"{}\" %u\nIcon=artisan-editor\nTerminal=false\n\
             Categories=Development;TextEditor;\nMimeType=x-scheme-handler/artisan;\nStartupWMClass=artisan-editor\n",
            editor.display()
        );
        sys::write(&desktop_file().map_err(IntegrateEditorError::Data)?, &entry)?;
        sys::write(
            &icon_file().map_err(IntegrateEditorError::Data)?,
            EDITOR_ICON,
        )?;
        if let Err(error) = sys::run(
            "xdg-mime",
            &["default", DESKTOP_FILE, "x-scheme-handler/artisan"],
        ) {
            tracing::debug!(
                "couldn't register artisan:// links: {}",
                diagnostics::chain(&error)
            );
        }
        Ok(())
    } else {
        Ok(())
    }
}

pub(crate) fn remove_editor_integration() -> Result<(), RemoveEditorIntegrationError> {
    if cfg!(windows) {
        sys::remove(
            &start_menu()
                .ok_or(RemoveEditorIntegrationError::NoAppData)?
                .join(SHORTCUT),
        )?;
        let registered = sys::powershell(&format!(
            "$v = (Get-ItemProperty -LiteralPath {key} -ErrorAction SilentlyContinue).'(default)'; if ($v) {{ $v }}",
            key = ps_string(&format!(r"{PROTOCOL_KEY}\shell\open\command")),
        ))
        .map_err(RemoveEditorIntegrationError::PowerShell)?;
        if registered.trim() == protocol_command(&paths::binary(App::Editor)?) {
            sys::powershell(&format!(
                "Remove-Item -LiteralPath {} -Recurse -Force",
                ps_string(PROTOCOL_KEY)
            ))
            .map_err(RemoveEditorIntegrationError::PowerShell)?;
        }
        Ok(())
    } else if cfg!(target_os = "linux") {
        sys::remove(&icon_file().map_err(RemoveEditorIntegrationError::Data)?)?;
        Ok(sys::remove(
            &desktop_file().map_err(RemoveEditorIntegrationError::Data)?,
        )?)
    } else {
        Ok(())
    }
}

fn protocol_command(editor: &Path) -> String {
    format!("\"{}\" \"%1\"", editor.display())
}

fn start_menu() -> Option<PathBuf> {
    let app_data = PathBuf::from(std::env::var_os("APPDATA")?);
    Some(app_data.join(r"Microsoft\Windows\Start Menu\Programs"))
}

fn desktop_file() -> Result<PathBuf, EnvPathError> {
    Ok(artisan_paths::xdg_data()?
        .join("applications")
        .join(DESKTOP_FILE))
}

fn icon_file() -> Result<PathBuf, EnvPathError> {
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

pub(crate) fn register_uninstall(app: App, version: &str) -> Result<(), RegisterUninstallError> {
    if !cfg!(windows) {
        return Ok(());
    }
    let name = match app {
        App::Ae => "Artisan command line (ae)",
        App::Forge => "Artisan Forge",
        App::Editor => "Artisan Editor",
    };
    let ae = paths::binary(App::Ae)?;
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
        location = ps_quote(&paths::dir(app)?),
        icon = ps_quote(&paths::binary(app)?),
        uninstall = ps_string(&uninstall),
    ))
    .map(drop)
    .map_err(|source| RegisterUninstallError::PowerShell { app, source })
}

pub(crate) fn unregister_uninstall(app: App) -> Result<(), RunError> {
    if !cfg!(windows) {
        return Ok(());
    }
    sys::powershell(&format!(
        "Remove-Item -Path {} -Recurse -Force -ErrorAction SilentlyContinue",
        ps_string(&uninstall_key(app))
    ))
    .map(drop)
}

pub(crate) fn launch_editor() -> Result<(), LaunchEditorError> {
    let editor = paths::binary(App::Editor)?;
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
pub(crate) struct Running {
    pub pid: u32,
    pub executable: PathBuf,
    pub started: u64,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum StopError {
    #[error("process {pid} ({}) still runs after it was force-stopped", executable.display())]
    Survived { pid: u32, executable: PathBuf },
}

pub(crate) fn close_processes(executables: &[PathBuf]) -> Result<usize, StopError> {
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

pub(crate) fn running(pid: u32) -> Option<Running> {
    processes(sysinfo::ProcessesToUpdate::Some(&[sysinfo::Pid::from_u32(
        pid,
    )]))
    .into_iter()
    .next()
}

pub(crate) fn stop(processes: &[Running]) -> Result<(), StopError> {
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

pub(crate) fn same_executable(left: &Path, right: &Path) -> bool {
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
            "asking the process to close failed: {}",
            diagnostics::chain(&error)
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
            "force-stopping the process failed: {}",
            diagnostics::chain(&error)
        );
    }
}

pub(crate) fn has_desktop() -> bool {
    cfg!(any(windows, target_os = "macos"))
        || std::env::var_os("DISPLAY").is_some()
        || std::env::var_os("WAYLAND_DISPLAY").is_some()
}

pub(crate) fn in_wsl() -> bool {
    cfg!(target_os = "linux") && std::env::var_os("WSL_DISTRO_NAME").is_some()
}
