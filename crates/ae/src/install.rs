use crate::release::{self, Carried, Feed, Release, Source};
use crate::{legacy, platform, service, sys, ui, wsl};
use anyhow::{Context, Result, bail};
use artisan_paths::App;
use clap::Args;
use std::path::{Path, PathBuf};

#[derive(Args, Default)]
pub struct InstallArgs {
    /// Where the Editor goes: here, windows (from WSL), keep or none.
    #[arg(long, value_name = "WHERE", value_parser = Place::parse)]
    editor: Option<Place>,
    /// Where Forge goes: here, wsl:<distro> (from Windows), keep or none.
    #[arg(long, value_name = "WHERE", value_parser = Place::parse)]
    forge: Option<Place>,
    /// Install this version instead of the newest (also how you go back).
    #[arg(long, value_name = "VERSION")]
    version: Option<String>,
    /// A release feed URL, a folder holding a release, or a local build.
    #[arg(long, value_name = "SOURCE")]
    from: Option<String>,
    /// The folder to install into instead of the default (first install only).
    #[arg(long, value_name = "FOLDER")]
    location: Option<PathBuf>,
    /// Don't ask to confirm the install (conflicts are still asked about
    /// in a terminal).
    #[arg(long, short)]
    yes: bool,
    #[arg(long, hide = true)]
    feed: Option<String>,
    #[arg(long, hide = true)]
    apply: bool,
    #[arg(long, hide = true)]
    bundled_editor: Option<PathBuf>,
    #[arg(long, hide = true)]
    bundled_linux_ae: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
    Here,
    Windows,
    Wsl(String),
    Keep,
    Skip,
}

impl Place {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "here" => Ok(Self::Here),
            "windows" => Ok(Self::Windows),
            "keep" => Ok(Self::Keep),
            "none" => Ok(Self::Skip),
            _ => match value.strip_prefix("wsl:") {
                Some(distro) if !distro.is_empty() => Ok(Self::Wsl(distro.to_owned())),
                _ => Err("expected here, windows, wsl:<distro>, keep or none".to_owned()),
            },
        }
    }

    fn argument(&self) -> String {
        match self {
            Self::Here => "here".to_owned(),
            Self::Windows => "windows".to_owned(),
            Self::Wsl(distro) => format!("wsl:{distro}"),
            Self::Keep => "keep".to_owned(),
            Self::Skip => "none".to_owned(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EditorTarget {
    Here,
    Windows,
    Nowhere,
}

#[derive(Debug, PartialEq, Eq)]
struct Plan {
    editor: EditorTarget,
    forge_here: bool,
    distros: Vec<String>,
}

impl Plan {
    fn settle(editor: &Place, forge: &Place) -> Self {
        let editor = match editor {
            Place::Here => EditorTarget::Here,
            Place::Windows => EditorTarget::Windows,
            Place::Keep if installed(App::Editor).is_some() => EditorTarget::Here,
            Place::Keep | Place::Wsl(_) | Place::Skip => EditorTarget::Nowhere,
        };
        let kept_here = installed(App::Forge).is_some();
        let (forge_here, distros) = match forge {
            Place::Here => (true, Vec::new()),
            Place::Wsl(distro) => (false, vec![distro.clone()]),
            Place::Keep if kept_here || !cfg!(windows) => (kept_here, Vec::new()),
            Place::Keep => (false, wsl::distros_with_forge()),
            Place::Windows | Place::Skip => (false, Vec::new()),
        };
        Self {
            editor,
            forge_here,
            distros,
        }
    }

    fn here(&self) -> Vec<App> {
        let mut apps = vec![App::Ae];
        if self.forge_here {
            apps.push(App::Forge);
        }
        if self.editor == EditorTarget::Here {
            apps.push(App::Editor);
        }
        apps
    }

    fn summary(&self, version: &str) -> Result<Vec<String>> {
        let mut lines = Vec::new();
        for app in self.here() {
            lines.push(summary_line(
                app,
                version,
                &app.dir()?.display().to_string(),
            ));
        }
        for distro in &self.distros {
            lines.push(format!("Forge    {version} → {distro} (WSL), with ae"));
        }
        if self.editor == EditorTarget::Windows {
            lines.push(format!("Editor   {version} → Windows, with ae"));
        }
        Ok(lines)
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

fn resolve(
    runtime: &tokio::runtime::Runtime,
    source: &Source,
    carried: Carried,
) -> Result<Release> {
    if matches!(source, Source::Bundled) {
        return runtime.block_on(release::resolve(source, carried));
    }
    let release = ui::task("Checking the release", "Checked the release", || {
        runtime.block_on(release::resolve(source, carried))
    })?;
    if release.is_local() {
        ui::info(format!(
            "Installing a local build from {} (not signed)",
            source.argument()
        ));
    }
    Ok(release)
}

fn relocate(location: &Path) -> Result<()> {
    let location =
        std::path::absolute(location).with_context(|| format!("resolve {}", location.display()))?;
    let current = artisan_paths::root()?;
    if location == current {
        return Ok(());
    }
    anyhow::ensure!(
        App::ALL.iter().all(|app| installed(*app).is_none()),
        "Artisan is already installed in {}; uninstall it before choosing another folder",
        current.display()
    );
    artisan_paths::choose_root(&location)
        .with_context(|| format!("remember {} as the install folder", location.display()))
}

pub fn install(args: &InstallArgs) -> Result<()> {
    if !args.apply {
        ui::intro("Artisan installer");
    }
    if args.editor == Some(Place::Windows) && !platform::in_wsl() {
        bail!("--editor windows is for installing from inside WSL");
    }
    if matches!(args.forge, Some(Place::Wsl(_))) && !cfg!(windows) {
        bail!("--forge wsl:<distro> is for installing from Windows");
    }
    let source = Source::new(args.from.as_deref(), args.version.as_deref(), args.apply)?;
    let runtime = runtime()?;
    let carried = Carried {
        editor: args.bundled_editor.clone(),
        linux_ae: args.bundled_linux_ae.clone(),
    };
    let release = resolve(&runtime, &source, carried)?;
    let version = release.version(os())?;
    if let Some(location) = &args.location {
        relocate(location)?;
    }

    let editor = match &args.editor {
        Some(place) => place.clone(),
        None if args.apply => return Err(ui::needs_flag("where the Editor goes", "--editor")),
        None => choose_editor(&release, &version)?,
    };
    let forge = match &args.forge {
        Some(place) => place.clone(),
        None if args.apply => return Err(ui::needs_flag("where Forge goes", "--forge")),
        None => choose_forge(&release, &version)?,
    };
    let plan = Plan::settle(&editor, &forge);

    if !args.apply {
        ui::note("Install", plan.summary(&version)?.join("\n"));
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
    }
    if let Some(file) = platform::foreign_ae() {
        let replace = if ui::interactive() && !args.apply {
            cliclack::confirm(format!(
                "{} is a file that isn't Artisan's. Replace it with Artisan's `ae`?",
                file.display()
            ))
            .initial_value(true)
            .interact()
            .map_err(ui::prompt_error)?
        } else if args.yes || args.apply {
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

    if let Release::Feed(feed) = &release {
        anyhow::ensure!(
            !args.apply,
            "a release feed installs through its own installer"
        );
        hand_over(&runtime, feed, &editor, &forge)?;
    } else {
        let feed = args
            .feed
            .clone()
            .or_else(|| source.update_feed().map(str::to_owned));
        apply(&release, &source, feed.as_deref(), &plan)?;
    }

    let editor_somewhere = plan.editor != EditorTarget::Nowhere;
    let forge_somewhere = plan.forge_here || !plan.distros.is_empty();
    ui::outro(if editor_somewhere && forge_somewhere {
        "Done. Pair the Editor with Forge from its Home screen."
    } else if forge_somewhere {
        "Done. Pair an Editor with `ae pair`."
    } else {
        "Done."
    });
    Ok(())
}

fn apply(release: &Release, source: &Source, feed: Option<&str>, plan: &Plan) -> Result<()> {
    let here = plan.here();
    legacy::migrate(&here)?;
    install_here(release, feed, &here)?;
    for distro in &plan.distros {
        install_in_wsl(release, source, feed, distro)?;
    }
    if plan.editor == EditorTarget::Windows {
        install_in_windows(release, source)?;
    }
    if plan.editor == EditorTarget::Here {
        match platform::launch_editor() {
            Ok(()) => ui::success("Opened the Editor"),
            Err(error) => ui::warning(format!("Couldn't open the Editor: {error:#}")),
        }
    }
    Ok(())
}

fn hand_over(
    runtime: &tokio::runtime::Runtime,
    feed: &Feed,
    editor: &Place,
    forge: &Place,
) -> Result<()> {
    let remembered: Vec<&str> = feed
        .update_feed()
        .map(|url| vec!["--feed", url])
        .unwrap_or_default();
    let mut editor = editor.clone();
    if editor == Place::Windows {
        let (folder, windows_folder) = windows_temp()?;
        let carrier = runtime.block_on(feed.fetch_carrier("windows", &folder))?;
        ui::step("Installing the Editor on Windows");
        let status = std::process::Command::new(&carrier)
            .args(["--install", "--editor", "here", "--forge", "none"])
            .args(&remembered)
            .current_dir("/mnt/c")
            .status()
            .with_context(|| format!("run {windows_folder}"))?;
        std::fs::remove_dir_all(&folder).ok();
        anyhow::ensure!(status.success(), "installing on Windows failed ({status})");
        editor = Place::Skip;
    }
    let folder = std::env::temp_dir().join(format!("artisan-install-{}", std::process::id()));
    std::fs::remove_dir_all(&folder).ok();
    let carrier = runtime.block_on(feed.fetch_carrier(os(), &folder))?;
    let mut command = std::process::Command::new(&carrier);
    if cfg!(windows) {
        command.arg("--install");
    } else {
        command.args(["install", "--apply"]);
    }
    let status = command
        .args(["--editor", &editor.argument()])
        .args(["--forge", &forge.argument()])
        .args(&remembered)
        .status()
        .with_context(|| format!("run {}", carrier.display()))?;
    std::fs::remove_dir_all(&folder).ok();
    anyhow::ensure!(
        status.success(),
        "installing Artisan {} failed ({status})",
        feed.version()
    );
    Ok(())
}

fn windows_temp() -> Result<(PathBuf, String)> {
    let temp = std::process::Command::new("cmd.exe")
        .args(["/d", "/c", "echo %TEMP%"])
        .current_dir("/mnt/c")
        .output()
        .context("find Windows' temp folder")?;
    let temp = String::from_utf8_lossy(&temp.stdout).trim().to_owned();
    let windows_folder = format!("{temp}\\artisan-install-{}", std::process::id());
    let folder = PathBuf::from(sys::output("wslpath", &["-a", "-u", &windows_folder])?.trim());
    std::fs::remove_dir_all(&folder).ok();
    Ok((folder, windows_folder))
}

fn remembered_feed() -> Result<String> {
    match std::fs::read_to_string(App::Ae.dir()?.join("release-feed")) {
        Ok(feed) => Ok(feed.trim().to_owned()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(release::feed_for(
            installed(App::Ae)
                .as_deref()
                .unwrap_or(env!("CARGO_PKG_VERSION")),
        )
        .to_owned()),
        Err(error) => Err(error).context("read the release feed"),
    }
}

pub fn install_editor() -> Result<()> {
    install(&InstallArgs {
        editor: Some(Place::Here),
        forge: Some(Place::Skip),
        from: Some(remembered_feed()?),
        yes: true,
        ..InstallArgs::default()
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

fn install_here(release: &Release, feed: Option<&str>, apps: &[App]) -> Result<()> {
    let version = release.version(os())?;
    for app in apps {
        let dir = app.dir()?;
        std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        clear_leftovers(&dir);
        let incoming = dir.join(".incoming");
        std::fs::remove_dir_all(&incoming).ok();
        release.unpack(*app, os(), &incoming)?;

        let progress =
            ui::Progress::spinner(format!("Installing {} {version}", app.display_name()));
        let result = (|| -> Result<String> {
            let binary = app.binary()?;
            #[cfg(unix)]
            let mut upgraded = false;
            #[cfg(not(unix))]
            let upgraded = false;
            match app {
                App::Forge => {
                    #[cfg(unix)]
                    {
                        if forge_http::upgrade::gateway(&App::Forge.data()?)?.is_some()
                            && service::is_running()
                        {
                            progress
                                .message("Checking replacement Forge and handing over new work");
                            runtime()?.block_on(forge_http::upgrade::replace(
                                &App::Forge.data()?,
                                &incoming.join("forge"),
                            ))?;
                            upgraded = true;
                        }
                    }
                    if !upgraded {
                        progress.message("Bootstrapping Forge's upgrade service");
                        service::prepare_bootstrap_stop()?;
                        service::stop()?;
                    }
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
            if let Some(feed) = feed {
                std::fs::write(dir.join("release-feed"), format!("{feed}\n"))
                    .context("remember the app's release feed")?;
            }
            platform::register_uninstall(*app, &version)?;
            Ok(match app {
                App::Ae => platform::add_to_path()?.map_or_else(
                    || format!("ae {version} installed, on PATH"),
                    |hint| format!("ae {version} installed. {hint}"),
                ),
                App::Forge => {
                    progress.message("Starting Forge");
                    let autostart = service::enable_autostart()?;
                    if !upgraded {
                        service::start()?;
                    }
                    format!(
                        "Forge {version} is running on {}; {autostart}",
                        service::address()
                    )
                }
                App::Editor => {
                    platform::integrate_editor()?;
                    format!("Editor {version} installed")
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

fn install_in_wsl(
    release: &Release,
    source: &Source,
    feed: Option<&str>,
    distro: &str,
) -> Result<()> {
    let stage = std::env::temp_dir().join(format!("artisan-wsl-{}", std::process::id()));
    std::fs::remove_dir_all(&stage).ok();
    release.unpack(App::Ae, "linux", &stage)?;
    let to_wsl = |path: &Path| -> Result<String> {
        let output = sys::quiet(
            std::process::Command::new("wsl.exe")
                .args(["-d", distro, "--exec", "wslpath", "-a", "-u"])
                .arg(path),
        )
        .output()
        .context("convert a path for WSL")?;
        anyhow::ensure!(output.status.success(), "wslpath failed in {distro}");
        Ok(String::from_utf8(output.stdout)?.trim().to_owned())
    };
    let mut origin = Vec::new();
    if let Source::Dir(dir) = source {
        origin.extend(["--from".to_owned(), to_wsl(dir)?]);
    }
    if let Some(feed) = feed {
        origin.extend(["--feed".to_owned(), feed.to_owned()]);
    }
    ui::step(format!("Installing Forge in {distro} (WSL)"));
    let status = sys::quiet(
        std::process::Command::new("wsl.exe")
            .args(["-d", distro, "--exec"])
            .arg(to_wsl(&stage.join("ae"))?)
            .args(["install", "--apply", "--forge", "here", "--editor", "none"])
            .args(origin),
    )
    .status()
    .context("run the installer in WSL")?;
    std::fs::remove_dir_all(&stage).ok();
    anyhow::ensure!(status.success(), "installing in {distro} failed ({status})");
    Ok(())
}

fn install_in_windows(release: &Release, source: &Source) -> Result<()> {
    let Source::Dir(dir) = source else {
        bail!("this installer carries no Editor for Windows");
    };
    let (stage, _) = windows_temp()?;
    release.unpack(App::Ae, "windows", &stage)?;
    let from = sys::output("wslpath", &["-a", "-w", &dir.to_string_lossy()])?
        .trim()
        .to_owned();
    ui::step("Installing the Editor on Windows");
    let status = std::process::Command::new(stage.join("ae.exe"))
        .args([
            "install", "--editor", "here", "--forge", "none", "--yes", "--from", &from,
        ])
        .current_dir("/mnt/c")
        .status()
        .context("run the installer on Windows")?;
    std::fs::remove_dir_all(&stage).ok();
    anyhow::ensure!(status.success(), "installing on Windows failed ({status})");
    Ok(())
}

pub fn update(yes: bool) -> Result<()> {
    ui::intro("Artisan update");
    let Some(current) = installed(App::Ae) else {
        bail!("nothing is installed here; run `ae install`");
    };
    let source = Source::new(Some(&remembered_feed()?), None, false)?;
    let runtime = runtime()?;
    let Release::Feed(feed) = resolve(&runtime, &source, Carried::default())? else {
        bail!("this install came from a local build; install again to change it");
    };
    let version = feed.version().to_owned();
    let outdated = release::newer(&version, &current);
    let lines: Vec<String> = App::ALL
        .into_iter()
        .filter_map(|app| {
            let current = installed(app)?;
            Some(if outdated {
                format!("{:<8} {current} → {version}", app.id())
            } else {
                format!("{:<8} {current} ✓ up to date", app.id())
            })
        })
        .collect();
    ui::note("Installed", lines.join("\n"));
    if !outdated {
        ui::outro("Everything is up to date.");
        return Ok(());
    }
    if !yes && ui::interactive() {
        let go = cliclack::confirm(format!("Update to {version}?"))
            .initial_value(true)
            .interact()
            .map_err(ui::prompt_error)?;
        if !go {
            ui::outro("Nothing was updated.");
            return Ok(());
        }
    }
    hand_over(&runtime, &feed, &Place::Keep, &Place::Keep)?;
    ui::outro("Up to date.");
    Ok(())
}

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
    if let Ok(root) = artisan_paths::root()
        && std::fs::remove_dir(root).is_ok()
    {
        artisan_paths::forget_root().ok();
    }
    Ok(())
}

#[cfg(test)]
#[path = "../tests/install/plan.rs"]
mod tests;
