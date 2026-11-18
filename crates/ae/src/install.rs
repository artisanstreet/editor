use crate::legacy::{self, MigrateError};
use crate::paths::{self, LocateError};
use crate::platform::{
    self, AddToPathError, IntegrateEditorError, RegisterUninstallError,
    RemoveEditorIntegrationError, RemoveFromPathError,
};
use crate::release::{
    self, Carried, Feed, FetchCarrierError, Release, ReleaseVersionError, ResolveError, Source,
    SourceError, UnpackError,
};
use crate::service::{AddressError, AutostartError, StartError, StopError};
use crate::sys::{self, RemoveError, RunError, SpawnError};
use crate::ui::{self, AskError};
use crate::{service, wsl};
use artisan_paths::{App, ChooseRootError, EnvPathError, RootError};
use clap::Args;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
#[cfg(unix)]
use std::time::Duration;

const ASIDE_SLOTS: usize = 100;
#[cfg(unix)]
const SCHEMA_PROBE_DEADLINE: Duration = Duration::from_secs(30);
const INSTALL_LOCK: &str = ".install.lock";
const ORIGIN_FILE: &str = "release-feed";

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("--editor windows is for installing from inside WSL")]
    EditorWindowsOutsideWsl,
    #[error("--forge wsl:<distro> is for installing from Windows")]
    ForgeWslOffWindows,
    #[error(transparent)]
    Source(#[from] SourceError),
    #[error("start the async runtime")]
    Runtime(#[source] io::Error),
    #[error(transparent)]
    Resolve(#[from] ResolveError),
    #[error(transparent)]
    Version(#[from] ReleaseVersionError),
    #[error("resolve {}", path.display())]
    Location {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("find the install folder")]
    Root(#[source] RootError),
    #[error("Artisan is already installed in {}; uninstall it before choosing another folder", root.display())]
    AlreadyInstalled { root: PathBuf },
    #[error(transparent)]
    Ask(#[from] AskError),
    #[error("Artisan {version} has no {} for {system}", app.display_name())]
    Missing {
        app: App,
        version: String,
        system: &'static str,
    },
    #[error("replace the `ae` that isn't Artisan's")]
    RemoveForeign(#[source] RemoveError),
    #[error("remember {} as the install folder", root.display())]
    ChooseRoot {
        root: PathBuf,
        #[source]
        source: ChooseRootError,
    },
    #[error("a release feed installs through its own installer")]
    FeedApply,
    #[error("{feed:?} is not a release feed URL")]
    NotAFeed { feed: String },
    #[error(transparent)]
    HandOver(#[from] HandOverError),
    #[error(transparent)]
    Apply(#[from] ApplyError),
}

#[derive(Debug, thiserror::Error)]
pub enum InstallLockError {
    #[error("find the install folder")]
    Root(#[source] RootError),
    #[error("create {}", path.display())]
    Create {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("lock {}", path.display())]
    Lock {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum StagingError {
    #[error("create {}", path.display())]
    Create {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("make a staging folder in {}", path.display())]
    Temp {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum WindowsTempError {
    #[error("find Windows' temp folder")]
    Cmd(#[source] io::Error),
    #[error("convert Windows' temp folder for WSL")]
    Convert(#[source] RunError),
    #[error("make a staging folder in {}", path.display())]
    Temp {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum HandOverError {
    #[error(transparent)]
    WindowsTemp(#[from] WindowsTempError),
    #[error(transparent)]
    Carrier(#[from] FetchCarrierError),
    #[error("run {}", program.display())]
    Run {
        program: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("installing Artisan {version} on Windows failed ({status})")]
    WindowsFailed { version: String, status: ExitStatus },
    #[error(transparent)]
    Locate(#[from] LocateError),
    #[error(transparent)]
    Staging(#[from] StagingError),
    #[error("installing Artisan {version} failed ({status})")]
    Failed { version: String, status: ExitStatus },
}

#[derive(Debug, thiserror::Error)]
pub enum RememberedFeedError {
    #[error(transparent)]
    Locate(#[from] LocateError),
    #[error("{} should name a release feed URL or `local` but holds {text:?}", path.display())]
    Unparseable { path: PathBuf, text: String },
    #[error("read {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("this install came from a local build; install again to change it")]
    Local,
}

#[derive(Debug, thiserror::Error)]
pub enum InstallEditorError {
    #[error(transparent)]
    Feed(#[from] RememberedFeedError),
    #[error(transparent)]
    Install(#[from] InstallError),
}

#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
    #[error(transparent)]
    Lock(#[from] InstallLockError),
    #[error(transparent)]
    Migrate(#[from] MigrateError),
    #[error(transparent)]
    Here(#[from] InstallHereError),
    #[error(transparent)]
    Wsl(#[from] InstallInWslError),
    #[error(transparent)]
    Windows(#[from] InstallInWindowsError),
}

#[derive(Debug, thiserror::Error)]
pub enum InstallHereError {
    #[error(transparent)]
    Version(#[from] ReleaseVersionError),
    #[error("install {} {version}", app.display_name())]
    App {
        app: App,
        version: String,
        #[source]
        source: Box<InstallAppError>,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum InstallAppError {
    #[error(transparent)]
    Locate(#[from] LocateError),
    #[error("create {}", path.display())]
    CreateDir {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("read {}", path.display())]
    Leftovers {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error(transparent)]
    Staging(#[from] StagingError),
    #[error(transparent)]
    Unpack(#[from] UnpackError),
    #[cfg(unix)]
    #[error(transparent)]
    Online(#[from] UpgradeOnlineError),
    #[error("prepare Forge's service to stop")]
    PrepareStop(#[source] AutostartError),
    #[error(transparent)]
    Stop(#[from] StopError),
    #[error("close the running Editor")]
    Close(#[source] platform::StopError),
    #[error(transparent)]
    Replace(#[from] ReplaceError),
    #[error("write {}", path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("write {}", path.display())]
    Notices {
        path: PathBuf,
        #[source]
        source: artisan_payload::WritePayloadError,
    },
    #[error(transparent)]
    Register(#[from] RegisterUninstallError),
    #[error(transparent)]
    Path(#[from] AddToPathError),
    #[error("make Forge start at login")]
    Autostart(#[source] AutostartError),
    #[error(transparent)]
    Start(#[from] StartError),
    #[error(transparent)]
    Address(#[from] AddressError),
    #[error(transparent)]
    Integrate(#[from] IntegrateEditorError),
}

#[cfg(unix)]
#[derive(Debug, thiserror::Error)]
pub enum UpgradeOnlineError {
    #[error(transparent)]
    Locate(#[from] LocateError),
    #[error("check whether Forge hands over to a new version")]
    Gateway(#[source] forge_http::upgrade::GatewayRecordError),
    #[error("read the schema version of Forge's database in {}", data.display())]
    Schema {
        data: PathBuf,
        #[source]
        source: forge_store::admin::AdminError,
    },
    #[error("run {} --schema-version", forge.display())]
    Probe {
        forge: PathBuf,
        #[source]
        source: sys::BoundedRunError,
    },
    #[error("{} --schema-version failed ({status})", forge.display())]
    ProbeFailed { forge: PathBuf, status: ExitStatus },
    #[error("read the schema version of {}", forge.display())]
    ProbeOutput {
        forge: PathBuf,
        #[source]
        source: std::num::ParseIntError,
    },
    #[error("start the async runtime")]
    Runtime(#[source] io::Error),
    #[error("hand Forge over to {}", forge.display())]
    Replace {
        forge: PathBuf,
        #[source]
        source: forge_http::upgrade::ReplaceError,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum InstallInWslError {
    #[error("make a staging folder")]
    Staging(#[source] io::Error),
    #[error(transparent)]
    Unpack(#[from] UnpackError),
    #[error("convert {} for WSL in {distro}", path.display())]
    Convert {
        distro: String,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("wslpath failed in {distro} for {} ({status})", path.display())]
    ConvertFailed {
        distro: String,
        path: PathBuf,
        status: ExitStatus,
    },
    #[error("wslpath in {distro} printed a path that isn't UTF-8")]
    NotUtf8 {
        distro: String,
        #[source]
        source: std::string::FromUtf8Error,
    },
    #[error("run the installer in {distro} (WSL)")]
    Run {
        distro: String,
        #[source]
        source: io::Error,
    },
    #[error("installing in {distro} failed ({status})")]
    Failed { distro: String, status: ExitStatus },
}

#[derive(Debug, thiserror::Error)]
pub enum InstallInWindowsError {
    #[error("this installer carries no Editor for Windows")]
    NotCarried,
    #[error(transparent)]
    Temp(#[from] WindowsTempError),
    #[error(transparent)]
    Unpack(#[from] UnpackError),
    #[error("convert {} for Windows", dir.display())]
    Convert {
        dir: PathBuf,
        #[source]
        source: RunError,
    },
    #[error("run the installer on Windows")]
    Run(#[source] io::Error),
    #[error("installing on Windows failed ({status})")]
    Failed { status: ExitStatus },
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("nothing is installed here; run `ae install`")]
    NothingInstalled,
    #[error(transparent)]
    Feed(#[from] RememberedFeedError),
    #[error(transparent)]
    Source(#[from] SourceError),
    #[error("start the async runtime")]
    Runtime(#[source] io::Error),
    #[error(transparent)]
    Resolve(#[from] ResolveError),
    #[error(transparent)]
    Ask(#[from] AskError),
    #[error(transparent)]
    HandOver(#[from] HandOverError),
}

#[derive(Debug, thiserror::Error)]
pub enum UninstallError {
    #[error(transparent)]
    Ask(#[from] AskError),
    #[error(
        "ae is what manages and uninstalls Forge and the Editor; uninstall them first, or together with ae"
    )]
    Orphans,
    #[error(transparent)]
    Remove(#[from] RemoveChosenError),
}

#[derive(Debug, thiserror::Error)]
pub enum RemoveChosenError {
    #[error(transparent)]
    Lock(#[from] InstallLockError),
    #[error("remove {}", app.display_name())]
    App {
        app: App,
        #[source]
        source: RemoveAppError,
    },
    #[error("remove the claude.ai browser profile")]
    BrowserProfile(#[source] RemoveBrowserProfileError),
    #[error(transparent)]
    Forget(#[from] ForgetRootError),
}

#[derive(Debug, thiserror::Error)]
pub enum RemoveBrowserProfileError {
    #[error("find the claude.ai browser profile")]
    Locate(#[source] RootError),
    #[error(transparent)]
    Remove(#[from] RemoveError),
}

#[derive(Debug, thiserror::Error)]
pub enum ForgetRootError {
    #[error("find the install folder")]
    Root(#[source] RootError),
    #[error("remove the install lock")]
    Lock(#[source] RemoveError),
    #[error(transparent)]
    RemoveDir(#[from] RemoveDirError),
    #[error("forget the install folder")]
    Forget(#[source] ChooseRootError),
}

#[derive(Debug, thiserror::Error)]
#[error("remove {}", path.display())]
pub struct RemoveDirError {
    path: PathBuf,
    #[source]
    source: io::Error,
}

#[derive(Debug, thiserror::Error)]
pub enum RemoveAppError {
    #[error(transparent)]
    Stop(#[from] StopError),
    #[error("stop Forge starting at login")]
    Autostart(#[source] AutostartError),
    #[error(transparent)]
    Locate(#[from] LocateError),
    #[error("close the running Editor")]
    Close(#[source] platform::StopError),
    #[error(transparent)]
    Integration(#[from] RemoveEditorIntegrationError),
    #[error(transparent)]
    Path(#[from] RemoveFromPathError),
    #[error("remove the Settings → Apps entry")]
    Unregister(#[source] RunError),
    #[error("read {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error(transparent)]
    Remove(#[from] RemoveError),
    #[error(transparent)]
    RemoveDir(#[from] RemoveDirError),
    #[error(transparent)]
    Later(#[from] RemoveSelfLaterError),
}

#[derive(Debug, thiserror::Error)]
pub enum RemoveSelfLaterError {
    #[error("find the install folder")]
    Root(#[source] RootError),
    #[error("find the default install folder")]
    Default(#[source] EnvPathError),
    #[error(transparent)]
    Spawn(#[from] SpawnError),
}

#[derive(Args, Default)]
pub struct InstallArgs {
    #[arg(long, value_name = "WHERE", value_parser = EditorPlace::parse, help = "Where the Editor goes: here, windows (from WSL), keep or none")]
    editor: Option<EditorPlace>,
    #[arg(long, value_name = "WHERE", value_parser = ForgePlace::parse, help = "Where Forge goes: here, wsl:<distro> (from Windows), keep or none")]
    forge: Option<ForgePlace>,
    #[arg(
        long,
        value_name = "VERSION",
        help = "Install this version instead of the newest (also how you go back)"
    )]
    version: Option<String>,
    #[arg(
        long,
        value_name = "SOURCE",
        help = "A release feed URL, a folder holding a release, or a local build"
    )]
    from: Option<String>,
    #[arg(
        long,
        value_name = "FOLDER",
        help = "The folder to install into instead of the default (first install only)"
    )]
    location: Option<PathBuf>,
    #[arg(
        long,
        short,
        help = "Don't ask to confirm the install (conflicts are still asked about in a terminal)"
    )]
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
pub enum EditorPlace {
    Here,
    Windows,
    Keep,
    Skip,
}

impl EditorPlace {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "here" => Ok(Self::Here),
            "windows" => Ok(Self::Windows),
            "keep" => Ok(Self::Keep),
            "none" => Ok(Self::Skip),
            _ => Err("the Editor goes here, windows (from WSL), keep or none".to_owned()),
        }
    }

    const fn argument(&self) -> &'static str {
        match self {
            Self::Here => "here",
            Self::Windows => "windows",
            Self::Keep => "keep",
            Self::Skip => "none",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ForgePlace {
    Here,
    Wsl(String),
    Keep,
    Skip,
}

impl ForgePlace {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "here" => Ok(Self::Here),
            "keep" => Ok(Self::Keep),
            "none" => Ok(Self::Skip),
            _ => match value.strip_prefix("wsl:") {
                Some(distro) if !distro.is_empty() => Ok(Self::Wsl(distro.to_owned())),
                _ => Err("Forge goes here, wsl:<distro> (from Windows), keep or none".to_owned()),
            },
        }
    }

    fn argument(&self) -> String {
        match self {
            Self::Here => "here".to_owned(),
            Self::Wsl(distro) => format!("wsl:{distro}"),
            Self::Keep => "keep".to_owned(),
            Self::Skip => "none".to_owned(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    Feed(String),
    Local,
}

impl Origin {
    fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let url = text
            .strip_prefix("https://")
            .or_else(|| text.strip_prefix("http://"));
        match url {
            _ if text == "local" => Some(Self::Local),
            Some(rest) if !rest.is_empty() && !text.contains(char::is_whitespace) => {
                Some(Self::Feed(text.to_owned()))
            }
            Some(_) | None => None,
        }
    }

    fn record(&self) -> String {
        match self {
            Self::Feed(url) => format!("{url}\n"),
            Self::Local => "local\n".to_owned(),
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
    fn settle(editor: &EditorPlace, forge: &ForgePlace) -> Self {
        let editor = match editor {
            EditorPlace::Here => EditorTarget::Here,
            EditorPlace::Windows => EditorTarget::Windows,
            EditorPlace::Keep if installed(App::Editor).is_some() => EditorTarget::Here,
            EditorPlace::Keep | EditorPlace::Skip => EditorTarget::Nowhere,
        };
        let (forge_here, distros) = match forge {
            ForgePlace::Here => (true, Vec::new()),
            ForgePlace::Wsl(distro) => (false, vec![distro.clone()]),
            ForgePlace::Keep => (installed(App::Forge).is_some(), kept_in_wsl()),
            ForgePlace::Skip => (false, Vec::new()),
        };
        Self {
            editor,
            forge_here,
            distros,
        }
    }

    fn check(&self, release: &Release, version: &str) -> Result<(), InstallError> {
        let missing = |app: App, system: &'static str| InstallError::Missing {
            app,
            version: version.to_owned(),
            system,
        };
        if self.forge_here && !release.has(App::Forge, os()) {
            return Err(missing(App::Forge, os()));
        }
        if self.editor == EditorTarget::Here && !release.has(App::Editor, os()) {
            return Err(missing(App::Editor, os()));
        }
        if self.editor == EditorTarget::Windows && !release.has(App::Editor, "windows") {
            return Err(missing(App::Editor, "windows"));
        }
        if !self.distros.is_empty() && !release.has(App::Forge, "linux") {
            return Err(missing(App::Forge, "WSL"));
        }
        Ok(())
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

    fn summary(&self, version: &str, root: &Path) -> Vec<String> {
        let mut lines = Vec::new();
        for app in self.here() {
            lines.push(summary_line(
                app,
                version,
                &app.dir_under(root).display().to_string(),
            ));
        }
        for distro in &self.distros {
            lines.push(format!("Forge    {version} → {distro} (WSL), with ae"));
        }
        if self.editor == EditorTarget::Windows {
            lines.push(format!("Editor   {version} → Windows, with ae"));
        }
        lines
    }
}

fn kept_in_wsl() -> Vec<String> {
    if !cfg!(windows) {
        return Vec::new();
    }
    match wsl::distros_with_forge() {
        Ok(distros) => distros,
        Err(error) => {
            ui::warning(format!(
                "Couldn't look for Forge in WSL: {}",
                diagnostics::chain(&error)
            ));
            Vec::new()
        }
    }
}

#[cfg(unix)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ForgeUpgrade {
    Online,
    Restart,
}

#[cfg(unix)]
const fn forge_upgrade(hands_over: bool, database: Option<usize>, incoming: usize) -> ForgeUpgrade {
    match database {
        Some(database) if hands_over && database == incoming => ForgeUpgrade::Online,
        Some(_) | None => ForgeUpgrade::Restart,
    }
}

const LICENSE: &str = include_str!("../../../LICENSE.md");

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

fn resolve(
    runtime: &tokio::runtime::Runtime,
    source: &Source,
    carried: Carried,
) -> Result<Release, ResolveError> {
    if matches!(source, Source::Bundled) {
        return runtime.block_on(release::resolve(source, carried, release::LIMITS));
    }
    let release = ui::task("Checking the release", "Checked the release", || {
        runtime.block_on(release::resolve(source, carried, release::LIMITS))
    })?;
    if release.is_local() {
        ui::info(format!(
            "Installing a local build from {} (not signed)",
            source.argument()
        ));
    }
    Ok(release)
}

fn relocation(location: &Path) -> Result<Option<PathBuf>, InstallError> {
    let location = std::path::absolute(location).map_err(|source| InstallError::Location {
        path: location.to_owned(),
        source,
    })?;
    let current = artisan_paths::root().map_err(InstallError::Root)?;
    if location == current {
        return Ok(None);
    }
    if App::ALL.iter().any(|app| installed(*app).is_some()) {
        return Err(InstallError::AlreadyInstalled { root: current });
    }
    Ok(Some(location))
}

fn install_lock() -> Result<std::fs::File, InstallLockError> {
    let root = artisan_paths::root().map_err(InstallLockError::Root)?;
    std::fs::create_dir_all(&root).map_err(|source| InstallLockError::Create {
        path: root.clone(),
        source,
    })?;
    let path = root.join(INSTALL_LOCK);
    let lock = |source| InstallLockError::Lock {
        path: path.clone(),
        source,
    };
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(lock)?;
    match file.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => {
            ui::info("Waiting for another installer to finish");
            file.lock().map_err(lock)?;
        }
        Err(std::fs::TryLockError::Error(error)) => return Err(lock(error)),
    }
    Ok(file)
}

fn staging_in(dir: &Path, prefix: &str) -> Result<tempfile::TempDir, StagingError> {
    std::fs::create_dir_all(dir).map_err(|source| StagingError::Create {
        path: dir.to_owned(),
        source,
    })?;
    tempfile::Builder::new()
        .prefix(prefix)
        .tempdir_in(dir)
        .map_err(|source| StagingError::Temp {
            path: dir.to_owned(),
            source,
        })
}

pub fn install(args: &InstallArgs) -> Result<(), InstallError> {
    if !args.apply {
        ui::intro("Artisan installer");
    }
    if args.editor == Some(EditorPlace::Windows) && !platform::in_wsl() {
        return Err(InstallError::EditorWindowsOutsideWsl);
    }
    if matches!(args.forge, Some(ForgePlace::Wsl(_))) && !cfg!(windows) {
        return Err(InstallError::ForgeWslOffWindows);
    }
    let source = Source::new(args.from.as_deref(), args.version.as_deref(), args.apply)?;
    let runtime = tokio::runtime::Runtime::new().map_err(InstallError::Runtime)?;
    let carried = Carried {
        editor: args.bundled_editor.clone(),
        linux_ae: args.bundled_linux_ae.clone(),
    };
    let release = resolve(&runtime, &source, carried)?;
    let version = release.version(os())?;
    let relocated = match &args.location {
        Some(location) => relocation(location)?,
        None => None,
    };
    let root = match &relocated {
        Some(root) => root.clone(),
        None => artisan_paths::root().map_err(InstallError::Root)?,
    };

    let editor = match &args.editor {
        Some(place) => place.clone(),
        None if args.apply => {
            return Err(ui::needs_flag("where the Editor goes", "--editor").into());
        }
        None => choose_editor(&release, &version)?,
    };
    let forge = match &args.forge {
        Some(place) => place.clone(),
        None if args.apply => return Err(ui::needs_flag("where Forge goes", "--forge").into()),
        None => choose_forge(&release, &version)?,
    };
    let plan = Plan::settle(&editor, &forge);
    plan.check(&release, &version)?;

    if !args.apply {
        ui::note("Install", plan.summary(&version, &root).join("\n"));
        if !args.yes && ui::interactive() && !confirmed("Install?")? {
            ui::outro("Nothing was installed.");
            return Ok(());
        }
    }
    if !replace_foreign_ae(args)? {
        ui::outro("Nothing was installed.");
        return Ok(());
    }
    if let Some(root) = &relocated {
        artisan_paths::choose_root(root).map_err(|source| InstallError::ChooseRoot {
            root: root.clone(),
            source,
        })?;
    }

    if let Release::Feed(feed) = &release {
        if args.apply {
            return Err(InstallError::FeedApply);
        }
        hand_over(&runtime, feed, &editor, &forge)?;
    } else {
        let origin = match args.feed.as_deref() {
            Some(feed) => Origin::parse(feed).ok_or_else(|| InstallError::NotAFeed {
                feed: feed.to_owned(),
            })?,
            None => source
                .update_feed()
                .map_or(Origin::Local, |feed| Origin::Feed(feed.to_owned())),
        };
        apply(&release, &source, &origin, &plan)?;
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

fn confirmed(question: &str) -> Result<bool, AskError> {
    cliclack::confirm(question)
        .initial_value(true)
        .interact()
        .map_err(ui::prompt_error)
}

fn replace_foreign_ae(args: &InstallArgs) -> Result<bool, InstallError> {
    let Some(file) = platform::foreign_ae() else {
        return Ok(true);
    };
    let replace = if ui::interactive() && !args.apply {
        confirmed(&format!(
            "{} is a file that isn't Artisan's. Replace it with Artisan's `ae`?",
            file.display()
        ))?
    } else if args.yes || args.apply {
        true
    } else {
        return Err(
            ui::needs_flag(&format!("whether to replace {}", file.display()), "--yes").into(),
        );
    };
    if replace {
        sys::remove(&file).map_err(InstallError::RemoveForeign)?;
    }
    Ok(replace)
}

fn apply(
    release: &Release,
    source: &Source,
    origin: &Origin,
    plan: &Plan,
) -> Result<(), ApplyError> {
    let _installing = install_lock()?;
    let here = plan.here();
    legacy::migrate(&here)?;
    install_here(release, origin, &here)?;
    for distro in &plan.distros {
        install_in_wsl(release, source, origin, distro)?;
    }
    if plan.editor == EditorTarget::Windows {
        install_in_windows(release, source)?;
    }
    if plan.editor == EditorTarget::Here {
        match platform::launch_editor() {
            Ok(()) => ui::success("Opened the Editor"),
            Err(error) => ui::warning(format!(
                "Couldn't open the Editor: {}",
                diagnostics::chain(&error)
            )),
        }
    }
    Ok(())
}

fn hand_over(
    runtime: &tokio::runtime::Runtime,
    feed: &Feed,
    editor: &EditorPlace,
    forge: &ForgePlace,
) -> Result<(), HandOverError> {
    let remembered: Vec<&str> = feed
        .update_feed()
        .map(|url| vec!["--feed", url])
        .unwrap_or_default();
    let mut editor = editor.clone();
    if editor == EditorPlace::Windows {
        let (staging, windows_folder) = windows_temp()?;
        let carrier = runtime.block_on(feed.fetch_carrier("windows", staging.path()))?;
        ui::step("Installing the Editor on Windows");
        let status = std::process::Command::new(&carrier)
            .args(["--install", "--editor", "here", "--forge", "none"])
            .args(&remembered)
            .current_dir("/mnt/c")
            .status()
            .map_err(|source| HandOverError::Run {
                program: PathBuf::from(windows_folder),
                source,
            })?;
        if !status.success() {
            return Err(HandOverError::WindowsFailed {
                version: feed.version().to_owned(),
                status,
            });
        }
        editor = EditorPlace::Skip;
    }
    let staging = staging_in(&paths::dir(App::Ae)?, ".installer-")?;
    let carrier = runtime.block_on(feed.fetch_carrier(os(), staging.path()))?;
    let mut command = std::process::Command::new(&carrier);
    if cfg!(windows) {
        command.arg("--install");
    } else {
        command.args(["install", "--apply"]);
    }
    let status = command
        .args(["--editor", editor.argument()])
        .args(["--forge", &forge.argument()])
        .args(&remembered)
        .status()
        .map_err(|source| HandOverError::Run {
            program: carrier.clone(),
            source,
        })?;
    if !status.success() {
        return Err(HandOverError::Failed {
            version: feed.version().to_owned(),
            status,
        });
    }
    Ok(())
}

fn windows_temp() -> Result<(tempfile::TempDir, String), WindowsTempError> {
    let temp = std::process::Command::new("cmd.exe")
        .args(["/d", "/c", "echo %TEMP%"])
        .current_dir("/mnt/c")
        .output()
        .map_err(WindowsTempError::Cmd)?;
    let temp = String::from_utf8_lossy(&temp.stdout).trim().to_owned();
    let folder = PathBuf::from(
        sys::output("wslpath", &["-a", "-u", &temp])
            .map_err(WindowsTempError::Convert)?
            .trim(),
    );
    let staging = tempfile::Builder::new()
        .prefix("artisan-install-")
        .tempdir_in(&folder)
        .map_err(|source| WindowsTempError::Temp {
            path: folder.clone(),
            source,
        })?;
    let windows_folder = sys::output("wslpath", &["-a", "-w", &staging.path().to_string_lossy()])
        .map_err(WindowsTempError::Convert)?
        .trim()
        .to_owned();
    Ok((staging, windows_folder))
}

fn origin_file() -> Result<PathBuf, LocateError> {
    Ok(paths::dir(App::Ae)?.join(ORIGIN_FILE))
}

fn remembered_origin() -> Result<Origin, RememberedFeedError> {
    let path = origin_file()?;
    match std::fs::read_to_string(&path) {
        Ok(text) => Origin::parse(&text).ok_or_else(|| RememberedFeedError::Unparseable {
            path,
            text: text.trim().to_owned(),
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Origin::Feed(
            release::feed_for(
                installed(App::Ae)
                    .as_deref()
                    .unwrap_or(env!("CARGO_PKG_VERSION")),
            )
            .to_owned(),
        )),
        Err(source) => Err(RememberedFeedError::Read { path, source }),
    }
}

fn remembered_feed() -> Result<String, RememberedFeedError> {
    match remembered_origin()? {
        Origin::Feed(url) => Ok(url),
        Origin::Local => Err(RememberedFeedError::Local),
    }
}

pub fn install_editor() -> Result<(), InstallEditorError> {
    Ok(install(&InstallArgs {
        editor: Some(EditorPlace::Here),
        forge: Some(ForgePlace::Skip),
        from: Some(remembered_feed()?),
        yes: true,
        ..InstallArgs::default()
    })?)
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

fn choose_editor(release: &Release, version: &str) -> Result<EditorPlace, AskError> {
    let wsl = platform::in_wsl();
    let mut options = Vec::new();
    if wsl && release.has(App::Editor, "windows") {
        options.push((EditorPlace::Windows, "On Windows".to_owned(), String::new()));
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
        options.push((EditorPlace::Here, label, version_hint(App::Editor, version)));
    }
    options.push((EditorPlace::Skip, "Don't install".to_owned(), String::new()));
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

fn choose_forge(release: &Release, version: &str) -> Result<ForgePlace, AskError> {
    let mut options = Vec::new();
    if release.has(App::Forge, os()) {
        options.push((
            ForgePlace::Here,
            format!("On {}", this_machine()),
            version_hint(App::Forge, version),
        ));
    }
    if cfg!(windows) && release.has(App::Forge, "linux") && ui::interactive() {
        match wsl::distros() {
            Ok(distros) => {
                for distro in distros {
                    options.push((
                        ForgePlace::Wsl(distro.clone()),
                        format!("In {distro} (WSL)"),
                        String::new(),
                    ));
                }
            }
            Err(error) => ui::warning(format!(
                "Couldn't list WSL distributions: {}",
                diagnostics::chain(&error)
            )),
        }
    }
    options.push((ForgePlace::Skip, "Don't install".to_owned(), String::new()));
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

#[cfg(unix)]
fn incoming_schema(forge: &Path) -> Result<usize, UpgradeOnlineError> {
    let output = sys::output_within(
        std::process::Command::new(forge).arg("--schema-version"),
        SCHEMA_PROBE_DEADLINE,
    )
    .map_err(|source| UpgradeOnlineError::Probe {
        forge: forge.to_owned(),
        source,
    })?;
    if !output.status.success() {
        return Err(UpgradeOnlineError::ProbeFailed {
            forge: forge.to_owned(),
            status: output.status,
        });
    }
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .map_err(|source| UpgradeOnlineError::ProbeOutput {
            forge: forge.to_owned(),
            source,
        })
}

#[cfg(unix)]
fn upgrade_forge_online(
    incoming: &Path,
    progress: &ui::Progress,
) -> Result<bool, UpgradeOnlineError> {
    let data = paths::data(App::Forge)?;
    let hands_over = forge_http::upgrade::gateway(&data)
        .map_err(UpgradeOnlineError::Gateway)?
        .is_some()
        && service::is_running();
    if !hands_over {
        return Ok(false);
    }
    let database =
        forge_store::admin::schema_version(&data).map_err(|source| UpgradeOnlineError::Schema {
            data: data.clone(),
            source,
        })?;
    let forge = incoming.join(App::Forge.binary_name());
    match forge_upgrade(hands_over, database, incoming_schema(&forge)?) {
        ForgeUpgrade::Online => {
            progress.message("Checking replacement Forge and handing over new work");
            tokio::runtime::Runtime::new()
                .map_err(UpgradeOnlineError::Runtime)?
                .block_on(forge_http::upgrade::replace(&data, &forge))
                .map_err(|source| UpgradeOnlineError::Replace { forge, source })?;
            Ok(true)
        }
        ForgeUpgrade::Restart => {
            progress.message("Forge's database changes in this version; restarting Forge");
            Ok(false)
        }
    }
}

fn install_here(release: &Release, origin: &Origin, apps: &[App]) -> Result<(), InstallHereError> {
    let version = release.version(os())?;
    for app in apps {
        install_app(release, origin, *app, &version).map_err(|source| InstallHereError::App {
            app: *app,
            version: version.clone(),
            source: Box::new(source),
        })?;
    }
    Ok(())
}

fn install_app(
    release: &Release,
    origin: &Origin,
    app: App,
    version: &str,
) -> Result<(), InstallAppError> {
    let dir = paths::dir(app)?;
    std::fs::create_dir_all(&dir).map_err(|source| InstallAppError::CreateDir {
        path: dir.clone(),
        source,
    })?;
    clear_leftovers(&dir)?;
    let incoming = staging_in(&dir, ".incoming-")?;
    release.unpack(app, os(), incoming.path())?;

    let progress = ui::Progress::spinner(format!("Installing {} {version}", app.display_name()));
    match put_in_place(origin, app, version, &dir, incoming.path(), &progress) {
        Ok(message) => {
            progress.done(message);
            Ok(())
        }
        Err(error) => {
            progress.fail(diagnostics::chain(&error));
            Err(error)
        }
    }
}

fn put_in_place(
    origin: &Origin,
    app: App,
    version: &str,
    dir: &Path,
    incoming: &Path,
    progress: &ui::Progress,
) -> Result<String, InstallAppError> {
    let binary = paths::binary(app)?;
    #[cfg(unix)]
    let upgraded = app == App::Forge && upgrade_forge_online(incoming, progress)?;
    #[cfg(not(unix))]
    let upgraded = false;
    match app {
        App::Forge if !upgraded => {
            progress.message("Stopping Forge");
            service::prepare_bootstrap_stop().map_err(InstallAppError::PrepareStop)?;
            service::stop()?;
        }
        App::Editor => {
            if platform::close_processes(std::slice::from_ref(&binary))
                .map_err(InstallAppError::Close)?
                > 0
            {
                progress.message("Closed the running Editor");
            }
        }
        App::Forge | App::Ae => {}
    }
    replace(dir, incoming)?;
    write_beside(dir, "LICENSE.md", LICENSE)?;
    if let Some(notices) = crate::bundle::notices(app) {
        let path = dir.join("NOTICES.txt");
        notices
            .write(&path)
            .map_err(|source| InstallAppError::Notices { path, source })?;
    }
    write_beside(dir, ORIGIN_FILE, &origin.record())?;
    platform::register_uninstall(app, version)?;
    let message = match app {
        App::Ae => platform::add_to_path()?.map_or_else(
            || format!("ae {version} installed, on PATH"),
            |hint| format!("ae {version} installed. {hint}"),
        ),
        App::Forge => {
            progress.message("Starting Forge");
            let autostart = service::enable_autostart().map_err(InstallAppError::Autostart)?;
            if !upgraded {
                service::start()?;
            }
            format!(
                "Forge {version} is running on {}; {autostart}",
                service::address()?
            )
        }
        App::Editor => {
            platform::integrate_editor()?;
            format!("Editor {version} installed")
        }
    };
    write_beside(dir, "version", &format!("{version}\n"))?;
    Ok(message)
}

fn write_beside(dir: &Path, name: &str, contents: &str) -> Result<(), InstallAppError> {
    let path = dir.join(name);
    std::fs::write(&path, contents).map_err(|source| InstallAppError::Write { path, source })
}

#[derive(Debug, thiserror::Error)]
pub enum ReplaceError {
    #[error("read {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("no free name to move {} aside among {ASIDE_SLOTS}", path.display())]
    NoAside { path: PathBuf },
    #[error("move {} aside", path.display())]
    Aside {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("put {} in place", path.display())]
    Place {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{failed}; restoring the previous files also failed: {restore}")]
    Restore {
        #[source]
        failed: Box<Self>,
        restore: String,
    },
}

struct Swap {
    target: PathBuf,
    aside: Option<PathBuf>,
    placed: bool,
}

fn replace(dir: &Path, incoming: &Path) -> Result<(), ReplaceError> {
    replace_with(dir, incoming, |from, to| std::fs::rename(from, to))
}

fn replace_with(
    dir: &Path,
    incoming: &Path,
    rename: impl Fn(&Path, &Path) -> io::Result<()>,
) -> Result<(), ReplaceError> {
    let read = |source| ReplaceError::Read {
        path: incoming.to_owned(),
        source,
    };
    let mut names = Vec::new();
    for entry in std::fs::read_dir(incoming).map_err(read)? {
        names.push(entry.map_err(read)?.file_name());
    }
    names.sort();
    let mut swaps = Vec::new();
    for name in &names {
        if let Err(error) = swap_in(dir, &incoming.join(name), name, &rename, &mut swaps) {
            return Err(roll_back(swaps, error, &rename));
        }
    }
    for swap in swaps {
        if let Some(aside) = swap.aside {
            discard(&aside);
        }
    }
    Ok(())
}

fn swap_in(
    dir: &Path,
    new: &Path,
    name: &std::ffi::OsStr,
    rename: &impl Fn(&Path, &Path) -> io::Result<()>,
    swaps: &mut Vec<Swap>,
) -> Result<(), ReplaceError> {
    let target = dir.join(name);
    let aside = if target.symlink_metadata().is_ok() {
        let aside = (0..ASIDE_SLOTS)
            .map(|slot| {
                let mut aside = std::ffi::OsString::from(format!(".old-{slot}-"));
                aside.push(name);
                dir.join(aside)
            })
            .find(|path| path.symlink_metadata().is_err())
            .ok_or_else(|| ReplaceError::NoAside {
                path: target.clone(),
            })?;
        rename(&target, &aside).map_err(|source| ReplaceError::Aside {
            path: target.clone(),
            source,
        })?;
        Some(aside)
    } else {
        None
    };
    swaps.push(Swap {
        target: target.clone(),
        aside,
        placed: false,
    });
    rename(new, &target).map_err(|source| ReplaceError::Place {
        path: target.clone(),
        source,
    })?;
    swaps
        .last_mut()
        .expect("invariant: the swap was just recorded")
        .placed = true;
    Ok(())
}

fn roll_back(
    swaps: Vec<Swap>,
    failed: ReplaceError,
    rename: &impl Fn(&Path, &Path) -> io::Result<()>,
) -> ReplaceError {
    let mut problems = Vec::new();
    for swap in swaps.into_iter().rev() {
        if swap.placed
            && let Err(error) = sys::remove(&swap.target)
        {
            problems.push(diagnostics::chain(&error));
            continue;
        }
        if let Some(aside) = swap.aside
            && let Err(error) = rename(&aside, &swap.target)
        {
            problems.push(format!(
                "put {} back as {}: {error}",
                aside.display(),
                swap.target.display()
            ));
        }
    }
    if problems.is_empty() {
        failed
    } else {
        ReplaceError::Restore {
            failed: Box::new(failed),
            restore: problems.join("; "),
        }
    }
}

fn discard(path: &Path) {
    if let Err(error) = sys::remove(path) {
        tracing::debug!(
            path = %path.display(),
            "the replaced file stays until the next install: {}",
            diagnostics::chain(&error)
        );
    }
}

fn clear_leftovers(dir: &Path) -> Result<(), InstallAppError> {
    let read = |source| InstallAppError::Leftovers {
        path: dir.to_owned(),
        source,
    };
    for entry in std::fs::read_dir(dir).map_err(read)? {
        let entry = entry.map_err(read)?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(".old-") || name.starts_with(".incoming-") {
            discard(&entry.path());
        }
    }
    Ok(())
}

fn install_in_wsl(
    release: &Release,
    source: &Source,
    origin: &Origin,
    distro: &str,
) -> Result<(), InstallInWslError> {
    let stage = tempfile::Builder::new()
        .prefix("artisan-wsl-")
        .tempdir()
        .map_err(InstallInWslError::Staging)?;
    release.unpack(App::Ae, "linux", stage.path())?;
    let to_wsl = |path: &Path| -> Result<String, InstallInWslError> {
        let output = sys::quiet(
            std::process::Command::new("wsl.exe")
                .args(["-d", distro, "--exec", "wslpath", "-a", "-u"])
                .arg(path),
        )
        .output()
        .map_err(|source| InstallInWslError::Convert {
            distro: distro.to_owned(),
            path: path.to_owned(),
            source,
        })?;
        if !output.status.success() {
            return Err(InstallInWslError::ConvertFailed {
                distro: distro.to_owned(),
                path: path.to_owned(),
                status: output.status,
            });
        }
        let converted =
            String::from_utf8(output.stdout).map_err(|source| InstallInWslError::NotUtf8 {
                distro: distro.to_owned(),
                source,
            })?;
        Ok(converted.trim().to_owned())
    };
    let mut forwarded = Vec::new();
    if let Source::Dir(dir) = source {
        forwarded.extend(["--from".to_owned(), to_wsl(dir)?]);
    }
    if let Origin::Feed(feed) = origin {
        forwarded.extend(["--feed".to_owned(), feed.clone()]);
    }
    ui::step(format!("Installing Forge in {distro} (WSL)"));
    let status = sys::quiet(
        std::process::Command::new("wsl.exe")
            .args(["-d", distro, "--exec"])
            .arg(to_wsl(&stage.path().join("ae"))?)
            .args(["install", "--apply", "--forge", "here", "--editor", "none"])
            .args(forwarded),
    )
    .status()
    .map_err(|source| InstallInWslError::Run {
        distro: distro.to_owned(),
        source,
    })?;
    if !status.success() {
        return Err(InstallInWslError::Failed {
            distro: distro.to_owned(),
            status,
        });
    }
    Ok(())
}

fn install_in_windows(release: &Release, source: &Source) -> Result<(), InstallInWindowsError> {
    let Source::Dir(dir) = source else {
        return Err(InstallInWindowsError::NotCarried);
    };
    let (stage, _) = windows_temp()?;
    release.unpack(App::Ae, "windows", stage.path())?;
    let from = sys::output("wslpath", &["-a", "-w", &dir.to_string_lossy()])
        .map_err(|source| InstallInWindowsError::Convert {
            dir: dir.clone(),
            source,
        })?
        .trim()
        .to_owned();
    ui::step("Installing the Editor on Windows");
    let status = std::process::Command::new(stage.path().join("ae.exe"))
        .args([
            "install", "--editor", "here", "--forge", "none", "--yes", "--from", &from,
        ])
        .current_dir("/mnt/c")
        .status()
        .map_err(InstallInWindowsError::Run)?;
    if !status.success() {
        return Err(InstallInWindowsError::Failed { status });
    }
    Ok(())
}

pub fn update(yes: bool) -> Result<(), UpdateError> {
    ui::intro("Artisan update");
    if installed(App::Ae).is_none() {
        return Err(UpdateError::NothingInstalled);
    }
    let source = Source::new(Some(&remembered_feed()?), None, false)?;
    let runtime = tokio::runtime::Runtime::new().map_err(UpdateError::Runtime)?;
    let Release::Feed(feed) = resolve(&runtime, &source, Carried::default())? else {
        return Err(RememberedFeedError::Local.into());
    };
    let version = feed.version().to_owned();
    let components: Vec<(App, String)> = App::ALL
        .into_iter()
        .filter_map(|app| Some((app, installed(app)?)))
        .collect();
    let outdated = components
        .iter()
        .any(|(_, current)| release::newer(&version, current));
    let lines: Vec<String> = components
        .iter()
        .map(|(app, current)| {
            if release::newer(&version, current) {
                format!("{:<8} {current} → {version}", app.id())
            } else {
                format!("{:<8} {current} ✓ up to date", app.id())
            }
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
    hand_over(&runtime, &feed, &EditorPlace::Keep, &ForgePlace::Keep)?;
    ui::outro("Up to date.");
    Ok(())
}

fn removable(present: &[App], with_data: &[App], purge: bool) -> Vec<App> {
    App::ALL
        .into_iter()
        .filter(|app| present.contains(app) || (purge && with_data.contains(app)))
        .collect()
}

fn leaves_orphans(chosen: &[App], present: &[App]) -> bool {
    chosen.contains(&App::Ae)
        && present
            .iter()
            .any(|app| *app != App::Ae && !chosen.contains(app))
}

pub fn uninstall(apps: &[App], purge: bool, yes: bool) -> Result<(), UninstallError> {
    ui::intro("Artisan uninstall");
    let present: Vec<App> = App::ALL
        .into_iter()
        .filter(|app| app.binary().is_ok_and(|binary| binary.is_file()))
        .collect();
    let with_data: Vec<App> = App::ALL
        .into_iter()
        .filter(|app| app.data().is_ok_and(|data| data.exists()))
        .collect();
    let candidates = removable(&present, &with_data, purge);
    let mut chosen: Vec<App> = if apps.is_empty() {
        candidates.clone()
    } else {
        apps.iter()
            .copied()
            .filter(|app| candidates.contains(app))
            .collect()
    };
    if chosen.is_empty() {
        ui::outro("Nothing to uninstall.");
        return Ok(());
    }
    let mut purge = purge;
    if !yes {
        if !ui::interactive() {
            return Err(ui::needs_flag("whether to uninstall", "--yes").into());
        }
        if apps.is_empty() {
            let mut select = cliclack::multiselect("Uninstall").required(true);
            for app in &candidates {
                select = select.item(
                    *app,
                    app.display_name(),
                    installed(*app).unwrap_or_default(),
                );
            }
            chosen = select
                .initial_values(candidates)
                .interact()
                .map_err(ui::prompt_error)?;
        }
        let keeping: Vec<&str> = chosen
            .iter()
            .filter(|app| with_data.contains(app))
            .map(|app| app.display_name())
            .collect();
        if !purge && !keeping.is_empty() {
            purge = cliclack::confirm(format!(
                "Also delete the data of {}? (threads, settings, paired devices)",
                keeping.join(" and ")
            ))
            .initial_value(false)
            .interact()
            .map_err(ui::prompt_error)?;
        }
    }
    if leaves_orphans(&chosen, &present) {
        return Err(UninstallError::Orphans);
    }
    if !yes {
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
    remove_chosen(chosen, purge)?;
    ui::outro(if purge {
        "Uninstalled."
    } else {
        "Uninstalled. Data was kept; `ae uninstall --purge` removes it."
    });
    Ok(())
}

fn remove_dir_if_empty(dir: &Path) -> Result<bool, RemoveDirError> {
    match std::fs::remove_dir(dir) {
        Ok(()) => Ok(true),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::DirectoryNotEmpty
            ) =>
        {
            Ok(false)
        }
        Err(source) => Err(RemoveDirError {
            path: dir.to_owned(),
            source,
        }),
    }
}

fn forget_empty_root() -> Result<(), ForgetRootError> {
    let root = artisan_paths::root().map_err(ForgetRootError::Root)?;
    sys::remove(&root.join(INSTALL_LOCK)).map_err(ForgetRootError::Lock)?;
    if remove_dir_if_empty(&root)? {
        artisan_paths::forget_root().map_err(ForgetRootError::Forget)?;
    }
    Ok(())
}

fn remove_chosen(mut chosen: Vec<App>, purge: bool) -> Result<(), RemoveChosenError> {
    let installing = install_lock()?;
    chosen.sort();
    chosen.reverse();
    let remaining: Vec<App> = App::ALL
        .into_iter()
        .filter(|app| !chosen.contains(app) && app.binary().is_ok_and(|binary| binary.is_file()))
        .collect();
    let mut deferred = false;
    for app in chosen {
        deferred |= ui::task(
            format!("Removing {}", app.display_name()),
            format!("Removed {}", app.display_name()),
            || remove_app(app, purge),
        )
        .map_err(|source| RemoveChosenError::App { app, source })?;
    }
    drop(installing);
    if purges_claude_ai_browser(purge, &remaining) {
        ui::task(
            "Removing the claude.ai browser profile",
            "Removed the claude.ai browser profile",
            remove_claude_ai_browser,
        )
        .map_err(RemoveChosenError::BrowserProfile)?;
    }
    if !deferred {
        forget_empty_root()?;
    }
    Ok(())
}

fn purges_claude_ai_browser(purge: bool, remaining: &[App]) -> bool {
    purge
        && !remaining
            .iter()
            .any(|app| matches!(app, App::Ae | App::Editor))
}

fn remove_claude_ai_browser() -> Result<(), RemoveBrowserProfileError> {
    Ok(sys::remove(
        &claude_ai_browser::profile_dir().map_err(RemoveBrowserProfileError::Locate)?,
    )?)
}

fn remove_app(app: App, purge: bool) -> Result<bool, RemoveAppError> {
    match app {
        App::Forge => {
            service::stop()?;
            service::disable_autostart().map_err(RemoveAppError::Autostart)?;
        }
        App::Editor => {
            platform::close_processes(&[paths::binary(app)?]).map_err(RemoveAppError::Close)?;
            platform::remove_editor_integration()?;
        }
        App::Ae => platform::remove_from_path()?,
    }
    platform::unregister_uninstall(app).map_err(RemoveAppError::Unregister)?;
    let dir = paths::dir(app)?;
    let binary = paths::binary(app)?;
    let self_removal = app == App::Ae && cfg!(windows);
    let read = |source| RemoveAppError::Read {
        path: dir.clone(),
        source,
    };
    for entry in std::fs::read_dir(&dir).map_err(read)? {
        let path = entry.map_err(read)?.path();
        if !purge && path.file_name().is_some_and(|name| name == "data") {
            continue;
        }
        if self_removal && path == binary {
            continue;
        }
        sys::remove(&path)?;
    }
    if self_removal {
        remove_self_later(&binary, &dir)?;
        return Ok(true);
    }
    if purge || app == App::Ae {
        remove_dir_if_empty(&dir)?;
    }
    Ok(false)
}

fn remove_self_later(ae: &Path, dir: &Path) -> Result<(), RemoveSelfLaterError> {
    let root = artisan_paths::root().map_err(RemoveSelfLaterError::Root)?;
    let script = format!(
        "ping -n 3 127.0.0.1 >nul & del /f /q \"{ae}\" & rmdir \"{dir}\" & del /f /q \"{lock}\" & rmdir \"{root}\" && (del /f /q \"{pointer}\" & rmdir \"{default}\")",
        ae = ae.display(),
        dir = dir.display(),
        lock = root.join(INSTALL_LOCK).display(),
        root = root.display(),
        pointer = artisan_paths::root_pointer()
            .map_err(RemoveSelfLaterError::Default)?
            .display(),
        default = artisan_paths::default_root()
            .map_err(RemoveSelfLaterError::Default)?
            .display(),
    );
    sys::spawn_detached(std::process::Command::new("cmd.exe").args(["/d", "/c", &script]))?;
    Ok(())
}

#[cfg(test)]
#[path = "../tests/install/plan.rs"]
mod tests;
