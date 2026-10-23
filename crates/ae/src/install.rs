//! `ae install | update | uninstall`. Each app has one installed version in
//! its own folder; installing replaces its files in place. `ae` comes with
//! every install, on every machine something is installed on.

use crate::release::{self, Release, Source};
use crate::{legacy, platform, service, sys, ui, wsl};
use anyhow::{Context, Result, bail};
use artisan_paths::App;
use clap::Args;
use std::path::{Path, PathBuf};

#[derive(Args)]
pub struct InstallArgs {
    /// Where the Editor goes: here, windows (from WSL) or none.
    #[arg(long, value_name = "WHERE", value_parser = Place::parse)]
    editor: Option<Place>,
    /// Where Forge goes: here, wsl:<distro> (from Windows) or none.
    #[arg(long, value_name = "WHERE", value_parser = Place::parse)]
    forge: Option<Place>,
    /// Install this version instead of the newest (also how you go back).
    #[arg(long, value_name = "VERSION")]
    version: Option<String>,
    /// A release feed URL, a folder holding a release, or a local build.
    #[arg(long, value_name = "SOURCE")]
    from: Option<String>,
    /// Don't ask to confirm the install (conflicts are still asked about
    /// in a terminal).
    #[arg(long, short)]
    yes: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
    Here,
    Windows,
    Wsl(String),
    Skip,
}

impl Place {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "here" => Ok(Self::Here),
            "windows" => Ok(Self::Windows),
            "none" => Ok(Self::Skip),
            _ => match value.strip_prefix("wsl:") {
                Some(distro) if !distro.is_empty() => Ok(Self::Wsl(distro.to_owned())),
                _ => Err("expected here, windows, wsl:<distro> or none".to_owned()),
            },
        }
    }
}

const fn os() -> &'static str {
    std::env::consts::OS
}

const fn this_machine() -> &'static str {
    if cfg!(windows) {
        "this PC"
    } else {
        "this machine"
    }
}

/// The version of `app` installed here.
pub fn installed(app: App) -> Option<String> {
    let dir = app.dir().ok()?;
    if !app.binary().ok()?.is_file() {
        return None;
    }
    let version = std::fs::read_to_string(dir.join("version")).ok()?;
    Some(version.trim().to_owned())
}

fn runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Runtime::new().context("start the async runtime")
}

fn resolve(runtime: &tokio::runtime::Runtime, source: &Source) -> Result<Release> {
    let release = ui::task("Checking the release", "Checked the release", || {
        runtime.block_on(release::resolve(source))
    })?;
    if release.is_local() {
        ui::info(format!(
            "Installing a local build from {} (not signed)",
            source.argument()
        ));
    }
    Ok(release)
}

pub fn install(args: &InstallArgs) -> Result<()> {
    ui::intro("Artisan installer");
    if args.editor == Some(Place::Windows) && !platform::in_wsl() {
        bail!("--editor windows is for installing from inside WSL");
    }
    if matches!(args.forge, Some(Place::Wsl(_))) && !cfg!(windows) {
        bail!("--forge wsl:<distro> is for installing from Windows");
    }
    let source = Source::new(args.from.as_deref(), args.version.as_deref())?;
    let runtime = runtime()?;
    let release = resolve(&runtime, &source)?;
    let version = release.version(os())?;

    let editor = match &args.editor {
        Some(place) => place.clone(),
        None => choose_editor(&release, &version)?,
    };
    let forge = match &args.forge {
        Some(place) => place.clone(),
        None => choose_forge(&release, &version)?,
    };

    let mut here = vec![App::Ae];
    if forge == Place::Here {
        here.push(App::Forge);
    }
    if editor == Place::Here {
        here.push(App::Editor);
    }
    let mut lines = Vec::new();
    for app in &here {
        lines.push(summary_line(
            *app,
            &version,
            &app.dir()?.display().to_string(),
        ));
    }
    if let Place::Wsl(distro) = &forge {
        lines.push(format!("Forge    {version} → {distro} (WSL), with ae"));
    }
    if editor == Place::Windows {
        lines.push(format!("Editor   {version} → Windows, with ae"));
    }
    ui::note("Install", lines.join("\n"));
    if !args.yes && ui::interactive() {
        let go = cliclack::confirm("Install?")
            .initial_value(true)
            .interact()
            .map_err(ui::prompt_error)?;
        if !go {
            ui::outro("Nothing was installed.");
            return Ok(());
        }
    }

    if let Some(file) = platform::foreign_ae() {
        // Conflicts are asked about even with --yes, which only skips the
        // summary's confirmation; without a terminal --yes replaces.
        let replace = if ui::interactive() {
            cliclack::confirm(format!(
                "{} is a file that isn't Artisan's. Replace it with Artisan's `ae`?",
                file.display()
            ))
            .initial_value(true)
            .interact()
            .map_err(ui::prompt_error)?
        } else if args.yes {
            true
        } else {
            return Err(ui::needs_flag(
                &format!("whether to replace {}", file.display()),
                "--yes",
            ));
        };
        if !replace {
            ui::outro("Nothing was installed.");
            return Ok(());
        }
        sys::remove(&file)?;
    }
    legacy::migrate(&here)?;
    install_here(&runtime, &release, &here)?;
    if let Place::Wsl(distro) = &forge {
        install_in_wsl(&runtime, &release, &source, distro)?;
    }
    if editor == Place::Windows {
        install_in_windows(&runtime, &release, &source)?;
    }

    let editor_somewhere = matches!(editor, Place::Here | Place::Windows);
    let forge_somewhere = matches!(forge, Place::Here | Place::Wsl(_));
    ui::outro(if editor_somewhere && forge_somewhere {
        "Done. Pair the Editor with Forge from its Home screen."
    } else if forge_somewhere {
        "Done. Pair an Editor with `ae pair`."
    } else {
        "Done."
    });
    Ok(())
}

/// Installs just the Editor here (`ae open` when it is missing).
pub fn install_editor() -> Result<()> {
    install(&InstallArgs {
        editor: Some(Place::Here),
        forge: Some(Place::Skip),
        version: None,
        from: None,
        yes: true,
    })
}

fn summary_line(app: App, version: &str, place: &str) -> String {
    let name = match app {
        App::Ae => "ae",
        App::Forge => "Forge",
        App::Editor => "Editor",
    };
    let change = match installed(app) {
        Some(old) if old == version => format!("{version} (reinstall)"),
        Some(old) => format!("{old} → {version}"),
        None => version.to_owned(),
    };
    format!("{name:<8} {change} → {place}")
}

fn version_hint(app: App, version: &str) -> String {
    match installed(app) {
        Some(old) if old == version => format!("{old} installed"),
        Some(old) => format!("{old} installed, updates to {version}"),
        None => String::new(),
    }
}

fn choose_editor(release: &Release, version: &str) -> Result<Place> {
    let wsl = platform::in_wsl();
    let mut options = Vec::new();
    if wsl && release.has(App::Editor, "windows") {
        options.push((Place::Windows, "On Windows".to_owned(), String::new()));
    }
    if platform::has_desktop() && release.has(App::Editor, os()) {
        let label = if wsl {
            format!(
                "In {} (WSL)",
                std::env::var("WSL_DISTRO_NAME").unwrap_or_default()
            )
        } else {
            format!("On {}", this_machine())
        };
        options.push((Place::Here, label, version_hint(App::Editor, version)));
    }
    options.push((Place::Skip, "Don't install".to_owned(), String::new()));
    let default = options[0].0.clone();
    if !ui::interactive() || options.len() == 1 {
        return Ok(default);
    }
    cliclack::select("Editor")
        .items(&options)
        .initial_value(default)
        .interact()
        .map_err(ui::prompt_error)
}

fn choose_forge(release: &Release, version: &str) -> Result<Place> {
    let mut options = Vec::new();
    if release.has(App::Forge, os()) {
        options.push((
            Place::Here,
            format!("On {}", this_machine()),
            version_hint(App::Forge, version),
        ));
    }
    if cfg!(windows) && release.has(App::Forge, "linux") && ui::interactive() {
        for distro in wsl::distros().unwrap_or_default() {
            options.push((
                Place::Wsl(distro.clone()),
                format!("In {distro} (WSL)"),
                String::new(),
            ));
        }
    }
    options.push((Place::Skip, "Don't install".to_owned(), String::new()));
    let default = options[0].0.clone();
    if !ui::interactive() || options.len() == 1 {
        return Ok(default);
    }
    cliclack::select("Forge")
        .items(&options)
        .initial_value(default)
        .interact()
        .map_err(ui::prompt_error)
}

/// Installs `apps` on this machine from `release`, in order.
fn install_here(runtime: &tokio::runtime::Runtime, release: &Release, apps: &[App]) -> Result<()> {
    let version = release.version(os())?;
    for app in apps {
        let dir = app.dir()?;
        std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        clear_leftovers(&dir);
        let incoming = dir.join(".incoming");
        std::fs::remove_dir_all(&incoming).ok();
        runtime.block_on(release.unpack(*app, os(), &incoming))?;

        let progress =
            ui::Progress::spinner(format!("Installing {} {version}", app.display_name()));
        let result = (|| -> Result<String> {
            let binary = app.binary()?;
            match app {
                App::Forge => {
                    progress.message("Stopping Forge");
                    service::stop()?;
                }
                App::Editor => {
                    if platform::close_processes(std::slice::from_ref(&binary))? > 0 {
                        progress.message("Closed the running Editor");
                    }
                }
                App::Ae => {}
            }
            replace(&dir, &incoming)?;
            std::fs::write(dir.join("version"), format!("{version}\n"))
                .with_context(|| format!("write {}", dir.join("version").display()))?;
            platform::register_uninstall(*app, &version)?;
            Ok(match app {
                App::Ae => platform::add_to_path()?.map_or_else(
                    || format!("ae {version} installed, on PATH"),
                    |hint| format!("ae {version} installed. {hint}"),
                ),

                App::Forge => {
                    progress.message("Starting Forge");
                    let autostart = service::enable_autostart()?;
                    service::start()?;
                    format!(
                        "Forge {version} is running on {}; {autostart}",
                        service::address()
                    )
                }
                App::Editor => {
                    platform::integrate_editor()?;
                    match platform::launch_editor() {
                        Ok(()) => format!("Editor {version} installed and opened"),
                        Err(error) => {
                            format!("Editor {version} installed (couldn't open it: {error:#})")
                        }
                    }
                }
            })
        })();
        match result {
            Ok(message) => progress.done(message),
            Err(error) => {
                progress.fail(format!("{error:#}"));
                return Err(error);
            }
        }
    }
    Ok(())
}

/// Moves everything in `incoming` into `dir`, replacing what is there. A
/// running executable can't be overwritten on Windows but can be renamed,
/// so it is moved aside first and deleted on the next install.
fn replace(dir: &Path, incoming: &Path) -> Result<()> {
    for entry in
        std::fs::read_dir(incoming).with_context(|| format!("read {}", incoming.display()))?
    {
        let entry = entry?;
        let target = dir.join(entry.file_name());
        if target.is_dir() {
            std::fs::remove_dir_all(&target)
                .with_context(|| format!("remove {}", target.display()))?;
        } else if target.exists() && cfg!(windows) {
            let aside = (0..100)
                .map(|index| {
                    dir.join(format!(
                        ".old-{index}-{}",
                        entry.file_name().to_string_lossy()
                    ))
                })
                .find(|path| !path.exists() || std::fs::remove_file(path).is_ok())
                .context("no free name to move the old file aside")?;
            std::fs::rename(&target, &aside)
                .with_context(|| format!("move {} aside", target.display()))?;
        }
        std::fs::rename(entry.path(), &target)
            .with_context(|| format!("put {} in place", target.display()))?;
    }
    std::fs::remove_dir_all(incoming).with_context(|| format!("remove {}", incoming.display()))
}

/// Files moved aside by earlier installs, once nothing runs them.
fn clear_leftovers(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with(".old-") {
            std::fs::remove_file(entry.path()).ok();
        }
    }
}

/// From Windows: runs the Linux `ae` from this release inside `distro` to
/// install Forge (and `ae`) there.
fn install_in_wsl(
    runtime: &tokio::runtime::Runtime,
    release: &Release,
    source: &Source,
    distro: &str,
) -> Result<()> {
    let stage = std::env::temp_dir().join(format!("artisan-wsl-{}", std::process::id()));
    std::fs::remove_dir_all(&stage).ok();
    runtime.block_on(release.unpack(App::Ae, "linux", &stage))?;
    let to_wsl = |path: &Path| -> Result<String> {
        let output = std::process::Command::new("wsl.exe")
            .args(["-d", distro, "--exec", "wslpath", "-a", "-u"])
            .arg(path)
            .output()
            .context("convert a path for WSL")?;
        anyhow::ensure!(output.status.success(), "wslpath failed in {distro}");
        Ok(String::from_utf8(output.stdout)?.trim().to_owned())
    };
    let from = match source {
        Source::Url(url) => url.clone(),
        Source::Dir(dir) => to_wsl(dir)?,
    };
    ui::step(format!("Installing Forge in {distro} (WSL)"));
    let status = std::process::Command::new("wsl.exe")
        .args(["-d", distro, "--exec"])
        .arg(to_wsl(&stage.join("ae"))?)
        .args([
            "install", "--forge", "here", "--editor", "none", "--yes", "--from", &from,
        ])
        .status()
        .context("run the installer in WSL")?;
    std::fs::remove_dir_all(&stage).ok();
    anyhow::ensure!(status.success(), "installing in {distro} failed ({status})");
    Ok(())
}

/// From WSL: runs the Windows `ae.exe` from this release to install the
/// Editor (and `ae`) on Windows.
fn install_in_windows(
    runtime: &tokio::runtime::Runtime,
    release: &Release,
    source: &Source,
) -> Result<()> {
    // Windows programs refuse a \\wsl.localhost working directory.
    let windows_cwd = Path::new("/mnt/c");
    let temp = std::process::Command::new("cmd.exe")
        .args(["/d", "/c", "echo %TEMP%"])
        .current_dir(windows_cwd)
        .output()
        .context("find Windows' temp folder")?;
    let temp = String::from_utf8_lossy(&temp.stdout).trim().to_owned();
    let windows_stage = format!("{temp}\\artisan-install-{}", std::process::id());
    let stage = PathBuf::from(sys::output("wslpath", &["-a", "-u", &windows_stage])?.trim());
    std::fs::remove_dir_all(&stage).ok();
    runtime.block_on(release.unpack(App::Ae, "windows", &stage))?;
    let from = match source {
        Source::Url(url) => url.clone(),
        Source::Dir(dir) => sys::output("wslpath", &["-a", "-w", &dir.to_string_lossy()])?
            .trim()
            .to_owned(),
    };
    ui::step("Installing the Editor on Windows");
    let status = std::process::Command::new(stage.join("ae.exe"))
        .args([
            "install", "--editor", "here", "--forge", "none", "--yes", "--from", &from,
        ])
        .current_dir(windows_cwd)
        .status()
        .context("run the installer on Windows")?;
    std::fs::remove_dir_all(&stage).ok();
    anyhow::ensure!(status.success(), "installing on Windows failed ({status})");
    Ok(())
}

/// `ae update [app…]`: the newest release for what is installed here (and
/// Forge in WSL, from Windows).
pub fn update(apps: &[App], yes: bool) -> Result<()> {
    ui::intro("Artisan update");
    let runtime = runtime()?;
    let release = resolve(&runtime, &Source::new(None, None)?)?;
    let version = release.version(os())?;
    let wanted: Vec<App> = if apps.is_empty() {
        App::ALL.to_vec()
    } else {
        apps.to_vec()
    };
    let mut lines = Vec::new();
    let mut outdated = Vec::new();
    for app in &wanted {
        let Some(current) = installed(*app) else {
            continue;
        };
        if release::newer(&version, &current) {
            lines.push(format!("{:<8} {current} → {version}", app.id()));
            outdated.push(*app);
        } else {
            lines.push(format!("{:<8} {current} ✓ up to date", app.id()));
        }
    }
    let distros =
        if cfg!(windows) && wanted.contains(&App::Forge) && !App::Forge.binary()?.is_file() {
            wsl::distros_with_forge()
        } else {
            Vec::new()
        };
    for distro in &distros {
        lines.push(format!("Forge in {distro} (WSL): updated by its own ae"));
    }
    if lines.is_empty() {
        bail!("nothing is installed here; run `ae install`");
    }
    ui::note("Installed", lines.join("\n"));
    if outdated.is_empty() && distros.is_empty() {
        ui::outro("Everything is up to date.");
        return Ok(());
    }
    if !yes && ui::interactive() && !outdated.is_empty() {
        let mut select = cliclack::multiselect("Update").required(false);
        for app in &outdated {
            select = select.item(*app, app.display_name(), "");
        }
        outdated = select
            .initial_values(outdated.clone())
            .interact()
            .map_err(ui::prompt_error)?;
    }
    outdated.sort();
    install_here(&runtime, &release, &outdated)?;
    for distro in &distros {
        ui::step(format!("Updating {distro} (WSL)"));
        let status = wsl::ae(distro)
            .args(["update", "--yes"])
            .status()
            .context("run wsl.exe")?;
        anyhow::ensure!(status.success(), "updating {distro} failed ({status})");
    }
    ui::outro("Up to date.");
    Ok(())
}

/// `ae uninstall [app…]`: removes the apps, their launcher entries and
/// services. Their data stays unless `purge`.
pub fn uninstall(apps: &[App], purge: bool, yes: bool) -> Result<()> {
    ui::intro("Artisan uninstall");
    let present: Vec<App> = App::ALL
        .into_iter()
        .filter(|app| app.binary().is_ok_and(|binary| binary.is_file()))
        .collect();
    let mut chosen: Vec<App> = if apps.is_empty() {
        present.clone()
    } else {
        apps.iter()
            .copied()
            .filter(|app| present.contains(app))
            .collect()
    };
    if chosen.is_empty() {
        ui::outro("Nothing to uninstall.");
        return Ok(());
    }
    let mut purge = purge;
    if !yes {
        if !ui::interactive() {
            return Err(ui::needs_flag("whether to uninstall", "--yes"));
        }
        if apps.is_empty() {
            let mut select = cliclack::multiselect("Uninstall").required(true);
            for app in &present {
                select = select.item(
                    *app,
                    app.display_name(),
                    installed(*app).unwrap_or_default(),
                );
            }
            chosen = select
                .initial_values(present)
                .interact()
                .map_err(ui::prompt_error)?;
        }
        let with_data: Vec<&str> = chosen
            .iter()
            .filter(|app| app.data().is_ok_and(|data| data.exists()))
            .map(|app| app.display_name())
            .collect();
        if !purge && !with_data.is_empty() {
            purge = cliclack::confirm(format!(
                "Also delete the data of {}? (threads, settings, paired devices)",
                with_data.join(" and ")
            ))
            .initial_value(false)
            .interact()
            .map_err(ui::prompt_error)?;
        }
        let names: Vec<&str> = chosen.iter().map(|app| app.display_name()).collect();
        let go = cliclack::confirm(format!("Uninstall {}?", names.join(", ")))
            .initial_value(true)
            .interact()
            .map_err(ui::prompt_error)?;
        if !go {
            ui::outro("Nothing was uninstalled.");
            return Ok(());
        }
    }
    chosen.sort();
    // ae last: it is what is running.
    chosen.reverse();
    for app in chosen {
        ui::task(
            format!("Removing {}", app.display_name()),
            format!("Removed {}", app.display_name()),
            || remove_app(app, purge),
        )?;
    }
    ui::outro(if purge {
        "Uninstalled."
    } else {
        "Uninstalled. Data was kept; `ae uninstall --purge` removes it."
    });
    Ok(())
}

fn remove_app(app: App, purge: bool) -> Result<()> {
    match app {
        App::Forge => {
            service::stop()?;
            service::disable_autostart()?;
        }
        App::Editor => {
            platform::close_processes(&[app.binary()?])?;
            platform::remove_editor_integration()?;
        }
        App::Ae => platform::remove_from_path()?,
    }
    platform::unregister_uninstall(app)?;
    let dir = app.dir()?;
    for entry in std::fs::read_dir(&dir).with_context(|| format!("read {}", dir.display()))? {
        let path = entry?.path();
        if !purge && path.file_name().is_some_and(|name| name == "data") {
            continue;
        }
        if app == App::Ae && cfg!(windows) && path == app.binary()? {
            // This ae.exe is running; Windows deletes it once it exits.
            let script = format!(
                "ping -n 3 127.0.0.1 >nul & del /f /q \"{}\"",
                path.display()
            );
            sys::spawn_detached(std::process::Command::new("cmd.exe").args(["/d", "/c", &script]))
                .ok();
            continue;
        }
        sys::remove(&path)?;
    }
    if purge || app == App::Ae {
        std::fs::remove_dir(&dir).ok();
    }
    if let Ok(root) = artisan_paths::root() {
        std::fs::remove_dir(root).ok();
    }
    Ok(())
}
