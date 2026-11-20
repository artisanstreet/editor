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
use swap::{JournalError, ReplaceError, RestoreFailures, Undone, Unreplaced, replace};

mod swap;
use artisan_paths::{App, ChooseRootError, EnvPathError, RootError};
use clap::Args;
use forge_store::admin::StorageVersion;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::time::Duration;

#[cfg(unix)]
const SCHEMA_PROBE_DEADLINE: Duration = Duration::from_secs(30);
const DATA_RELEASE_DEADLINE: Duration = Duration::from_secs(120);
const INSTALL_LOCK: &str = ".install.lock";
const ORIGIN_FILE: &str = "release-feed";

#[derive(Debug, thiserror::Error)]
pub(crate) enum InstallError {
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
    Installed(#[from] InstalledError),
    #[error(transparent)]
    Ask(#[from] AskError),
    #[error("Artisan {version} has no {} for {system}", app.display_name())]
    Missing {
        app: App,
        version: String,
        system: &'static str,
    },
    #[error(transparent)]
    PlaceAe(#[from] PlaceAeError),
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
    #[error(transparent)]
    Recover(#[from] RecoverError),
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum InstallLockError {
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
pub(crate) enum StagingError {
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
pub(crate) enum WindowsTempError {
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
pub(crate) enum HandOverError {
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
pub(crate) enum RememberedFeedError {
    #[error(transparent)]
    Locate(#[from] LocateError),
    #[error(transparent)]
    Installed(#[from] InstalledError),
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
pub(crate) enum InstallEditorError {
    #[error(transparent)]
    Feed(#[from] RememberedFeedError),
    #[error(transparent)]
    Install(#[from] InstallError),
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ApplyError {
    #[error(transparent)]
    Lock(#[from] InstallLockError),
    #[error(transparent)]
    Migrate(#[from] MigrateError),
    #[error(transparent)]
    Here(#[from] InstallHereError),
    #[error("remove the earlier install after installing over it")]
    Retire(#[source] MigrateError),
    #[error(transparent)]
    Wsl(#[from] InstallInWslError),
    #[error(transparent)]
    Windows(#[from] InstallInWindowsError),
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum InstallHereError {
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
pub(crate) enum InstallAppError {
    #[error(transparent)]
    Locate(#[from] LocateError),
    #[error("read the storage version of Forge's database in {} before stopping it", data.display())]
    DatabaseVersion {
        data: PathBuf,
        #[source]
        source: forge_store::admin::StorageVersionError,
    },
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
    #[error("the install failed and {outcome}")]
    PutBack {
        #[source]
        failed: Box<Self>,
        outcome: PutBack,
    },
    #[error("the new files are in place, but the next install would put the previous ones back")]
    Settle(#[source] JournalError),
    #[error("{failed}; the next install would also put the previous files back: {}", diagnostics::chain(.settle))]
    Unsettled {
        #[source]
        failed: Box<Self>,
        settle: JournalError,
    },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum RecoverError {
    #[error(transparent)]
    Locate(#[from] LocateError),
    #[error(transparent)]
    Lock(#[from] InstallLockError),
    #[error(
        "finish what an interrupted install of {} left behind; to give up on it, run `ae uninstall {}`, or delete {} and install again",
        app.display_name(),
        app.id(),
        journal.display()
    )]
    Interrupted {
        app: App,
        journal: PathBuf,
        #[source]
        source: Box<RecoverFailure>,
    },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum RecoverFailure {
    #[error(transparent)]
    Files(#[from] swap::RecoverError),
    #[error("stop the Forge the interrupted install started")]
    PrepareStop(#[source] AutostartError),
    #[error("stop the Forge the interrupted install started")]
    Stop(#[source] StopError),
    #[error("the previous Forge's files are back, but its database couldn't be put back")]
    Database(#[source] Box<RestoreDatabaseError>),
    #[error("the previous files are back, but the record of the interrupted install stays")]
    Settle(#[source] JournalError),
}

#[derive(Debug)]
pub(crate) enum PutBack {
    NotRestored(Box<RestoreFailures>),
    DatabaseNotRestored(Box<RestoreDatabaseError>),
    Settled(Settled),
}

#[derive(Debug)]
pub(crate) enum Settled {
    Restored,
    Restarted,
    NotRestarted(Box<StartError>),
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum RestoreDatabaseError {
    #[error(transparent)]
    Locate(#[from] LocateError),
    #[error("stop the Forge that failed to start")]
    Stop(#[source] StopError),
    #[error("take Forge's data folder to restore its database")]
    Lock(#[source] forge_store::WaitForDataError),
    #[error("put back the schema {schema} database the previous Forge used in {}", data.display())]
    RollBack {
        data: PathBuf,
        schema: usize,
        #[source]
        source: Box<forge_store::RollbackError>,
    },
    #[error("set aside the database in {} that the previous Forge never had", data.display())]
    Clear {
        data: PathBuf,
        #[source]
        source: Box<forge_store::RollbackError>,
    },
}

impl std::fmt::Display for PutBack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotRestored(failures) => write!(
                f,
                "putting the previous version back also failed: {}",
                diagnostics::chain(failures)
            ),
            Self::DatabaseNotRestored(error) => write!(
                f,
                "the previous Forge is back in place but its database couldn't be put back, so it wasn't started: {}",
                diagnostics::chain(error)
            ),
            Self::Settled(settled) => settled.fmt(f),
        }
    }
}

impl std::fmt::Display for Settled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Restored => f.write_str("the previous version is back in place"),
            Self::Restarted => f.write_str("the previous Forge is back in place and running"),
            Self::NotRestarted(error) => write!(
                f,
                "the previous Forge is back in place but didn't start: {}",
                diagnostics::chain(error)
            ),
        }
    }
}

#[cfg(unix)]
#[derive(Debug, thiserror::Error)]
pub(crate) enum UpgradeOnlineError {
    #[error(transparent)]
    Locate(#[from] LocateError),
    #[error("check whether Forge is running")]
    Running(#[source] service::RunningError),
    #[error("check whether Forge hands over to a new version")]
    Gateway(#[source] forge_http::upgrade::GatewayRecordError),
    #[error("run {} --schema-version", forge.display())]
    Probe {
        forge: PathBuf,
        #[source]
        source: sys::BoundedRunError,
    },
    #[error("{} --schema-version failed ({status})", forge.display())]
    ProbeFailed { forge: PathBuf, status: ExitStatus },
    #[error("read the storage version of {}", forge.display())]
    ProbeOutput {
        forge: PathBuf,
        #[source]
        source: serde_json::Error,
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
pub(crate) enum InstallInWslError {
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
    #[error("check for an ae that isn't Artisan's in {distro} (WSL)")]
    Check {
        distro: String,
        #[source]
        source: io::Error,
    },
    #[error("checking for an ae that isn't Artisan's in {distro} (WSL) failed ({status})")]
    CheckFailed { distro: String, status: ExitStatus },
    #[error(transparent)]
    Ask(#[from] AskError),
    #[error(
        "left ~/.local/bin/ae in {distro} (WSL) alone, so Forge wasn't installed there: the Editor starts Forge through that path"
    )]
    ForeignAeKept { distro: String },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum InstallInWindowsError {
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
pub(crate) enum UpdateError {
    #[error("nothing is installed here; run `ae install`")]
    NothingInstalled,
    #[error(transparent)]
    Installed(#[from] InstalledError),
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
    PlaceAe(#[from] PlaceAeError),
    #[error(transparent)]
    HandOver(#[from] HandOverError),
    #[error(transparent)]
    Recover(#[from] RecoverError),
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum UninstallError {
    #[error(transparent)]
    Ask(#[from] AskError),
    #[error(transparent)]
    Installed(#[from] InstalledError),
    #[error("check whether {} has data in {}", app.display_name(), path.display())]
    Data {
        app: App,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("find {}'s data folder", app.display_name())]
    DataFolder {
        app: App,
        #[source]
        source: RootError,
    },
    #[error(
        "ae is what manages and uninstalls Forge and the Editor; uninstall them first, or together with ae"
    )]
    Orphans,
    #[error(transparent)]
    Remove(#[from] RemoveChosenError),
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum RemoveChosenError {
    #[error(transparent)]
    Lock(#[from] InstallLockError),
    #[error(transparent)]
    Installed(#[from] InstalledError),
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
pub(crate) enum RemoveBrowserProfileError {
    #[error("find the claude.ai browser profile")]
    Locate(#[source] RootError),
    #[error(transparent)]
    Remove(#[from] RemoveError),
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ForgetRootError {
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
pub(crate) struct RemoveDirError {
    path: PathBuf,
    #[source]
    source: io::Error,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum RemoveAppError {
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
pub(crate) enum RemoveSelfLaterError {
    #[error("find the install folder")]
    Root(#[source] RootError),
    #[error("find the default install folder")]
    Default(#[source] EnvPathError),
    #[error(transparent)]
    Spawn(#[from] SpawnError),
}

#[derive(Args, Default)]
pub(crate) struct InstallArgs {
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
    #[arg(long, hide = true, value_name = "PATH", requires = "replace_ae_found")]
    replace_ae: Option<PathBuf>,
    #[arg(long, hide = true, value_name = "WHAT", value_parser = platform::Found::parse, requires = "replace_ae")]
    replace_ae_found: Option<platform::Found>,
}

impl InstallArgs {
    fn agreed(&self) -> Option<platform::Consent> {
        Some(platform::Consent {
            path: self.replace_ae.clone()?,
            found: self.replace_ae_found.clone()?,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum EditorPlace {
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
pub(crate) enum ForgePlace {
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
pub(crate) enum Origin {
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
    fn settle(editor: &EditorPlace, forge: &ForgePlace) -> Result<Self, InstalledError> {
        let editor = match editor {
            EditorPlace::Here => EditorTarget::Here,
            EditorPlace::Windows => EditorTarget::Windows,
            EditorPlace::Keep => {
                if installed(App::Editor)?.is_present() {
                    EditorTarget::Here
                } else {
                    EditorTarget::Nowhere
                }
            }
            EditorPlace::Skip => EditorTarget::Nowhere,
        };
        let (forge_here, distros) = match forge {
            ForgePlace::Here => (true, Vec::new()),
            ForgePlace::Wsl(distro) => (false, vec![distro.clone()]),
            ForgePlace::Keep => (installed(App::Forge)?.is_present(), kept_in_wsl()),
            ForgePlace::Skip => (false, Vec::new()),
        };
        Ok(Self {
            editor,
            forge_here,
            distros,
        })
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
        let mut apps = Vec::new();
        if self.forge_here {
            apps.push(App::Forge);
        }
        apps.push(App::Ae);
        if self.editor == EditorTarget::Here {
            apps.push(App::Editor);
        }
        apps
    }

    fn summary(&self, version: &str, root: &Path) -> Result<Vec<String>, InstalledError> {
        let mut lines = Vec::new();
        for app in self.here() {
            lines.push(summary_line(
                app,
                &installed(app)?,
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
        Ok(lines)
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
fn forge_upgrade(
    hands_over: bool,
    database: Option<StorageVersion>,
    incoming: StorageVersion,
) -> ForgeUpgrade {
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Installed {
    Absent,
    Present(InstalledVersion),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum InstalledVersion {
    Known(String),
    Missing,
    Unreadable { path: PathBuf, kind: io::ErrorKind },
}

impl Installed {
    pub(crate) const fn is_present(&self) -> bool {
        match self {
            Self::Absent => false,
            Self::Present(_) => true,
        }
    }
}

impl std::fmt::Display for InstalledVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Known(version) => f.write_str(version),
            Self::Missing | Self::Unreadable { .. } => f.write_str("unknown version"),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum InstalledError {
    #[error("find where {} is installed", app.display_name())]
    Locate {
        app: App,
        #[source]
        source: RootError,
    },
    #[error("check whether {} is installed at {}", app.display_name(), path.display())]
    Binary {
        app: App,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

pub(crate) fn installed(app: App) -> Result<Installed, InstalledError> {
    let locate = |source| InstalledError::Locate { app, source };
    let binary = app.binary().map_err(locate)?;
    match std::fs::metadata(&binary) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_not_a_file) => return Ok(Installed::Absent),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Installed::Absent),
        Err(source) => {
            return Err(InstalledError::Binary {
                app,
                path: binary,
                source,
            });
        }
    }
    let path = app.dir().map_err(locate)?.join("version");
    match std::fs::read_to_string(&path) {
        Ok(version) => Ok(Installed::Present(InstalledVersion::Known(
            version.trim().to_owned(),
        ))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Ok(Installed::Present(InstalledVersion::Missing))
        }
        Err(error) => Ok(Installed::Present(InstalledVersion::Unreadable {
            path,
            kind: error.kind(),
        })),
    }
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
    for app in App::ALL {
        if installed(app)?.is_present() {
            return Err(InstallError::AlreadyInstalled { root: current });
        }
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

fn recover_interrupted() -> Result<(), RecoverError> {
    let mut interrupted = Vec::new();
    for app in App::ALL {
        let dir = paths::dir(app)?;
        if swap::interrupted(&dir).map_err(|source| RecoverError::Interrupted {
            app,
            journal: swap::journal(&dir),
            source: Box::new(source.into()),
        })? {
            interrupted.push((app, dir));
        }
    }
    if interrupted.is_empty() {
        return Ok(());
    }
    let _installing = install_lock()?;
    for (app, dir) in interrupted {
        recover_app(app, &dir).map_err(|source| RecoverError::Interrupted {
            app,
            journal: swap::journal(&dir),
            source: Box::new(source),
        })?;
    }
    Ok(())
}

fn recover_app(app: App, dir: &Path) -> Result<(), RecoverFailure> {
    let Some(pending) = swap::pending::<Before>(dir)? else {
        return Ok(());
    };
    let progress = ui::Progress::spinner(format!(
        "Finishing an interrupted install of {}",
        app.display_name()
    ));
    let recovered = recover_pending(pending, &progress);
    match &recovered {
        Ok(settled) => progress.done(format!(
            "Put back the {} files an interrupted install left half replaced; {settled}",
            app.display_name()
        )),
        Err(error) => progress.fail(diagnostics::chain(error)),
    }
    recovered.map(drop)
}

fn recover_pending(
    pending: swap::Pending<Before>,
    progress: &ui::Progress,
) -> Result<Settled, RecoverFailure> {
    match pending.before() {
        Before::Forge { .. } => {
            progress.message("Stopping the Forge the interrupted install started");
            service::prepare_bootstrap_stop().map_err(RecoverFailure::PrepareStop)?;
            service::stop().map_err(RecoverFailure::Stop)?;
        }
        Before::Files => {}
    }
    let undone = pending.put_back()?;
    let (settled, forgotten) = settle(undone, progress).map_err(RecoverFailure::Database)?;
    forgotten.map_err(RecoverFailure::Settle)?;
    Ok(settled)
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

pub(crate) fn install(args: &InstallArgs) -> Result<(), InstallError> {
    if !args.apply {
        ui::intro("Artisan installer");
    }
    if args.editor == Some(EditorPlace::Windows) && !platform::in_wsl() {
        return Err(InstallError::EditorWindowsOutsideWsl);
    }
    if matches!(args.forge, Some(ForgePlace::Wsl(_))) && !cfg!(windows) {
        return Err(InstallError::ForgeWslOffWindows);
    }
    recover_interrupted()?;
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
    let plan = Plan::settle(&editor, &forge)?;
    plan.check(&release, &version)?;

    if !args.apply {
        ui::note("Install", plan.summary(&version, &root)?.join("\n"));
        if !args.yes && ui::interactive() && !confirmed("Install?")? {
            ui::outro("Nothing was installed.");
            return Ok(());
        }
    }
    let Some(placement) = place_ae(args.yes, args.apply, args.agreed().as_ref())? else {
        ui::outro("Nothing was installed.");
        return Ok(());
    };
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
        let forwarded = Forwarded {
            yes: args.yes,
            replace_ae: placement.agreed,
        };
        hand_over(&runtime, feed, &editor, &forge, &forwarded)?;
    } else {
        let origin = match args.feed.as_deref() {
            Some(feed) => Origin::parse(feed).ok_or_else(|| InstallError::NotAFeed {
                feed: feed.to_owned(),
            })?,
            None => source
                .update_feed()
                .map_or(Origin::Local, |feed| Origin::Feed(feed.to_owned())),
        };
        apply(
            &release,
            &source,
            &origin,
            &plan,
            args.yes,
            &placement.replaceable,
        )?;
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

struct AePlacement {
    replaceable: platform::Replaceable,
    agreed: Option<platform::Consent>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum PlaceAeError {
    #[error("look for an `ae` that isn't Artisan's")]
    Existing(#[from] platform::ExistingAeError),
    #[error(transparent)]
    Ask(#[from] AskError),
}

fn place_ae(
    yes: bool,
    apply: bool,
    agreed: Option<&platform::Consent>,
) -> Result<Option<AePlacement>, PlaceAeError> {
    let (file, replaceable) = match platform::existing_ae()? {
        platform::ExistingAe::Artisans(replaceable) => {
            return Ok(Some(AePlacement {
                replaceable,
                agreed: None,
            }));
        }
        platform::ExistingAe::Foreign { path, replaceable } => (path, replaceable),
    };
    let found = replaceable.consent(&file);
    let replace = if agreed == Some(&found) {
        true
    } else if ui::interactive() && !apply {
        confirmed(&format!(
            "{} is a file that isn't Artisan's. Replace it with Artisan's `ae`?",
            file.display()
        ))?
    } else if yes {
        true
    } else {
        return Err(
            ui::needs_flag(&format!("whether to replace {}", file.display()), "--yes").into(),
        );
    };
    Ok(replace.then_some(AePlacement {
        replaceable,
        agreed: Some(found),
    }))
}

#[derive(Debug)]
struct Forwarded {
    yes: bool,
    replace_ae: Option<platform::Consent>,
}

impl Forwarded {
    fn arguments(&self) -> Vec<std::ffi::OsString> {
        let mut arguments = Vec::new();
        if self.yes {
            arguments.push("--yes".into());
        }
        if let Some(consent) = &self.replace_ae {
            arguments.push("--replace-ae".into());
            arguments.push(consent.path.clone().into_os_string());
            arguments.push("--replace-ae-found".into());
            arguments.push(consent.found.argument().into());
        }
        arguments
    }
}

fn apply(
    release: &Release,
    source: &Source,
    origin: &Origin,
    plan: &Plan,
    yes: bool,
    replaceable: &platform::Replaceable,
) -> Result<(), ApplyError> {
    let _installing = install_lock()?;
    let here = plan.here();
    let earlier = legacy::migrate(&here)?;
    install_here(release, origin, &here, replaceable)?;
    earlier.retire().map_err(ApplyError::Retire)?;
    for distro in &plan.distros {
        install_in_wsl(release, source, origin, distro, yes)?;
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
    forwarded: &Forwarded,
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
        .args(forwarded.arguments())
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
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let ours = match installed(App::Ae)? {
                Installed::Present(InstalledVersion::Known(version)) => version,
                Installed::Present(
                    InstalledVersion::Missing | InstalledVersion::Unreadable { .. },
                )
                | Installed::Absent => env!("CARGO_PKG_VERSION").to_owned(),
            };
            Ok(Origin::Feed(release::feed_for(&ours).to_owned()))
        }
        Err(source) => Err(RememberedFeedError::Read { path, source }),
    }
}

fn remembered_feed() -> Result<String, RememberedFeedError> {
    match remembered_origin()? {
        Origin::Feed(url) => Ok(url),
        Origin::Local => Err(RememberedFeedError::Local),
    }
}

pub(crate) fn install_editor() -> Result<(), InstallEditorError> {
    Ok(install(&InstallArgs {
        editor: Some(EditorPlace::Here),
        forge: Some(ForgePlace::Skip),
        from: Some(remembered_feed()?),
        yes: true,
        ..InstallArgs::default()
    })?)
}

fn summary_line(app: App, current: &Installed, version: &str, place: &str) -> String {
    let name = match app {
        App::Ae => "ae",
        App::Forge => "Forge",
        App::Editor => "Editor",
    };
    let change = match current {
        Installed::Present(InstalledVersion::Known(old)) if old == version => {
            format!("{version} (reinstall)")
        }
        Installed::Present(old) => format!("{old} → {version}"),
        Installed::Absent => version.to_owned(),
    };
    format!("{name:<8} {change} → {place}")
}

fn version_hint(app: App, version: &str) -> Result<String, InstalledError> {
    Ok(match installed(app)? {
        Installed::Present(InstalledVersion::Known(old)) if old == version => {
            format!("{old} installed")
        }
        Installed::Present(old) => format!("{old} installed, updates to {version}"),
        Installed::Absent => String::new(),
    })
}

fn choose_editor(release: &Release, version: &str) -> Result<EditorPlace, InstallError> {
    let wsl = platform::in_wsl();
    let mut options = Vec::new();
    if wsl && release.has(App::Editor, "windows") {
        options.push((EditorPlace::Windows, "On Windows".to_owned(), String::new()));
    }
    if platform::has_desktop() && release.has(App::Editor, os()) {
        let label = if wsl {
            match std::env::var_os("WSL_DISTRO_NAME") {
                Some(distro) => format!("In {} (WSL)", distro.to_string_lossy()),
                None => "In WSL".to_owned(),
            }
        } else {
            format!("On {}", this_machine())
        };
        options.push((
            EditorPlace::Here,
            label,
            version_hint(App::Editor, version)?,
        ));
    }
    options.push((EditorPlace::Skip, "Don't install".to_owned(), String::new()));
    let default = options[0].0.clone();
    if !ui::interactive() || options.len() == 1 {
        return Ok(default);
    }
    Ok(cliclack::select("Editor")
        .items(&options)
        .initial_value(default)
        .interact()
        .map_err(ui::prompt_error)?)
}

fn choose_forge(release: &Release, version: &str) -> Result<ForgePlace, InstallError> {
    let mut options = Vec::new();
    if release.has(App::Forge, os()) {
        options.push((
            ForgePlace::Here,
            format!("On {}", this_machine()),
            version_hint(App::Forge, version)?,
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
    Ok(cliclack::select("Forge")
        .items(&options)
        .initial_value(default)
        .interact()
        .map_err(ui::prompt_error)?)
}

#[cfg(unix)]
fn incoming_schema(forge: &Path) -> Result<StorageVersion, UpgradeOnlineError> {
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
    serde_json::from_slice(&output.stdout).map_err(|source| UpgradeOnlineError::ProbeOutput {
        forge: forge.to_owned(),
        source,
    })
}

#[cfg(unix)]
fn upgrade_forge_online(
    incoming: &Path,
    database: Option<StorageVersion>,
    progress: &ui::Progress,
) -> Result<bool, UpgradeOnlineError> {
    let data = paths::data(App::Forge)?;
    let hands_over = forge_http::upgrade::gateway(&data)
        .map_err(UpgradeOnlineError::Gateway)?
        .is_some()
        && service::is_running().map_err(UpgradeOnlineError::Running)?;
    if !hands_over {
        return Ok(false);
    }
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

fn install_here(
    release: &Release,
    origin: &Origin,
    apps: &[App],
    replaceable: &platform::Replaceable,
) -> Result<(), InstallHereError> {
    let version = release.version(os())?;
    for app in apps {
        install_app(release, origin, *app, &version, replaceable).map_err(|source| {
            InstallHereError::App {
                app: *app,
                version: version.clone(),
                source: Box::new(source),
            }
        })?;
    }
    Ok(())
}

fn install_app(
    release: &Release,
    origin: &Origin,
    app: App,
    version: &str,
    replaceable: &platform::Replaceable,
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
    match put_in_place(
        origin,
        app,
        version,
        &dir,
        incoming.path(),
        replaceable,
        &progress,
    ) {
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
    replaceable: &platform::Replaceable,
    progress: &ui::Progress,
) -> Result<String, InstallAppError> {
    let binary = paths::binary(app)?;
    let database = match app {
        App::Forge => {
            let data = paths::data(App::Forge)?;
            Some(
                forge_store::admin::storage_version(&data).map_err(|source| {
                    InstallAppError::DatabaseVersion {
                        data: data.clone(),
                        source,
                    }
                })?,
            )
        }
        App::Editor | App::Ae => None,
    };
    #[cfg(unix)]
    let upgraded = match database {
        Some(database) => upgrade_forge_online(incoming, database, progress)?,
        None => false,
    };
    #[cfg(not(unix))]
    let upgraded = false;
    write_beside(incoming, "LICENSE.md", LICENSE)?;
    if let Some(notices) = crate::bundle::notices(app) {
        let path = incoming.join("NOTICES.txt");
        notices
            .write(&path)
            .map_err(|source| InstallAppError::Notices { path, source })?;
    }
    write_beside(incoming, ORIGIN_FILE, &origin.record())?;
    write_beside(incoming, "version", &format!("{version}\n"))?;
    let before = database.map_or(Before::Files, |database| Before::Forge {
        database: StoredBefore::of(database),
    });
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
    let replaced = match replace(dir, incoming, before) {
        Ok(replaced) => replaced,
        Err(Unreplaced::Undone { failed, undone }) => {
            return Err(put_back((*failed).into(), undone, progress));
        }
        Err(Unreplaced::Broken { failed, restore }) => {
            return Err(ReplaceError::Restore { failed, restore }.into());
        }
    };
    match finish(app, version, upgraded, replaceable, progress) {
        Ok(message) => {
            replaced.keep().map_err(InstallAppError::Settle)?;
            Ok(message)
        }
        Err(failed) if upgraded => Err(match replaced.keep() {
            Ok(()) => failed,
            Err(settle) => InstallAppError::Unsettled {
                failed: Box::new(failed),
                settle,
            },
        }),
        Err(failed) => {
            progress.message("Putting the previous version back");
            match replaced.undo() {
                Ok(undone) => Err(put_back(failed, undone, progress)),
                Err(failures) => Err(InstallAppError::PutBack {
                    failed: Box::new(failed),
                    outcome: PutBack::NotRestored(Box::new(failures)),
                }),
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum Before {
    Files,
    Forge { database: StoredBefore },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum StoredBefore {
    Nothing,
    Schema(usize),
}

impl StoredBefore {
    const fn of(database: Option<StorageVersion>) -> Self {
        match database {
            Some(StorageVersion { schema, .. }) if schema > 0 => Self::Schema(schema),
            Some(_) | None => Self::Nothing,
        }
    }
}

fn put_back(
    failed: InstallAppError,
    undone: Undone<Before>,
    progress: &ui::Progress,
) -> InstallAppError {
    let failed = Box::new(failed);
    match settle(undone, progress) {
        Err(kept) => InstallAppError::PutBack {
            failed,
            outcome: PutBack::DatabaseNotRestored(kept),
        },
        Ok((settled, Ok(()))) => InstallAppError::PutBack {
            failed,
            outcome: PutBack::Settled(settled),
        },
        Ok((settled, Err(settle))) => InstallAppError::Unsettled {
            failed: Box::new(InstallAppError::PutBack {
                failed,
                outcome: PutBack::Settled(settled),
            }),
            settle,
        },
    }
}

fn settle(
    undone: Undone<Before>,
    progress: &ui::Progress,
) -> Result<(Settled, Result<(), JournalError>), Box<RestoreDatabaseError>> {
    let settled = match *undone.before() {
        Before::Files => Settled::Restored,
        Before::Forge { database } => match restore_database(database, progress) {
            Err(error) => return Err(Box::new(error)),
            Ok(()) if undone.put_back_previous(&App::Forge.binary_name()) => {
                progress.message("Starting the previous Forge again");
                match service::start(service::Readiness::Listening) {
                    Ok(_) => Settled::Restarted,
                    Err(error) => Settled::NotRestarted(Box::new(error)),
                }
            }
            Ok(()) => Settled::Restored,
        },
    };
    Ok((settled, undone.forget()))
}

fn restore_database(
    database: StoredBefore,
    progress: &ui::Progress,
) -> Result<(), RestoreDatabaseError> {
    service::stop().map_err(RestoreDatabaseError::Stop)?;
    let data = paths::data(App::Forge)?;
    let lock = forge_store::DataLock::acquire_waiting(
        &data,
        &forge_store::Patience::system(DATA_RELEASE_DEADLINE),
        || progress.message("Waiting for the Forge that didn't start to let go of its data"),
    )
    .map_err(RestoreDatabaseError::Lock)?;
    match database {
        StoredBefore::Schema(schema) => {
            match forge_store::admin::roll_back_for(&lock, schema).map_err(|source| {
                RestoreDatabaseError::RollBack {
                    data: data.clone(),
                    schema,
                    source: Box::new(source),
                }
            })? {
                forge_store::admin::RolledBack::Current => {}
                forge_store::admin::RolledBack::Restored {
                    found,
                    restored,
                    aside,
                } => progress.message(format!(
                    "Put back the schema {restored} database for the previous Forge; the schema {found} data is kept in {}",
                    aside.display()
                )),
            }
        }
        StoredBefore::Nothing => {
            match forge_store::admin::clear_for_a_fresh_start(&lock).map_err(|source| {
                RestoreDatabaseError::Clear {
                    data: data.clone(),
                    source: Box::new(source),
                }
            })? {
                forge_store::admin::Cleared::Nothing => {}
                forge_store::admin::Cleared::SetAside { found, aside } => {
                    progress.message(format!(
                        "The previous Forge had no database yet, so it starts without one; the schema {found} data is kept in {}",
                        aside.display()
                    ));
                }
            }
        }
    }
    Ok(())
}

fn finish(
    app: App,
    version: &str,
    upgraded: bool,
    replaceable: &platform::Replaceable,
    progress: &ui::Progress,
) -> Result<String, InstallAppError> {
    platform::register_uninstall(app, version)?;
    Ok(match app {
        App::Ae => platform::add_to_path(replaceable)?.map_or_else(
            || format!("ae {version} installed, on PATH"),
            |hint| format!("ae {version} installed. {hint}"),
        ),
        App::Forge => {
            progress.message("Starting Forge");
            let autostart = service::enable_autostart().map_err(InstallAppError::Autostart)?;
            if !upgraded {
                service::start(service::Readiness::Recorded)?;
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
    })
}

fn write_beside(dir: &Path, name: &str, contents: &str) -> Result<(), InstallAppError> {
    let path = dir.join(name);
    std::fs::write(&path, contents).map_err(|source| InstallAppError::Write { path, source })
}

fn clear_leftovers(dir: &Path) -> Result<(), InstallAppError> {
    let read = |source| InstallAppError::Leftovers {
        path: dir.to_owned(),
        source,
    };
    for entry in std::fs::read_dir(dir).map_err(read)? {
        let entry = entry.map_err(read)?;
        let name = entry.file_name();
        if swap::leftover(&name) || name.to_string_lossy().starts_with(".incoming-") {
            swap::discard(&entry.path());
        }
    }
    Ok(())
}

const FOREIGN_AE_IN_WSL: &str = concat!(
    r#"link="$HOME/.local/bin/ae"; "#,
    r#"test -e "$link" || test -L "$link" || exit 1; "#,
    r#"test -L "$link" || exit 0; "#,
    r#"default="${XDG_DATA_HOME:-$HOME/.local/share}/artisan-street"; "#,
    r#"root="$default"; "#,
    r#"if test -e "$default/location"; then IFS= read -r root < "$default/location"; fi; "#,
    r#"test "$(readlink "$link")" = "${root%/}/ae/ae" && exit 1; "#,
    r#"exit 0"#,
);

fn replaces_foreign_ae_in(distro: &str, yes: bool) -> Result<bool, InstallInWslError> {
    let status = sys::quiet(std::process::Command::new("wsl.exe").args([
        "-d",
        distro,
        "--exec",
        "sh",
        "-c",
        FOREIGN_AE_IN_WSL,
    ]))
    .status()
    .map_err(|source| InstallInWslError::Check {
        distro: distro.to_owned(),
        source,
    })?;
    match status.code() {
        Some(1) => Ok(false),
        Some(0) if ui::interactive() => {
            let question = format!(
                "~/.local/bin/ae in {distro} (WSL) is a file that isn't Artisan's. Replace it with Artisan's `ae`?"
            );
            if cliclack::confirm(question)
                .initial_value(true)
                .interact()
                .map_err(ui::prompt_error)?
            {
                Ok(true)
            } else {
                Err(InstallInWslError::ForeignAeKept {
                    distro: distro.to_owned(),
                })
            }
        }
        Some(0) if yes => Ok(true),
        Some(0) => Err(InstallInWslError::Ask(ui::needs_flag(
            &format!("whether to replace ~/.local/bin/ae in {distro} (WSL)"),
            "--yes",
        ))),
        _ => Err(InstallInWslError::CheckFailed {
            distro: distro.to_owned(),
            status,
        }),
    }
}

fn install_in_wsl(
    release: &Release,
    source: &Source,
    origin: &Origin,
    distro: &str,
    yes: bool,
) -> Result<(), InstallInWslError> {
    let replaces_foreign_ae = replaces_foreign_ae_in(distro, yes)?;
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
    if replaces_foreign_ae {
        forwarded.push("--yes".to_owned());
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

pub(crate) fn update(yes: bool) -> Result<(), UpdateError> {
    ui::intro("Artisan update");
    recover_interrupted()?;
    if !installed(App::Ae)?.is_present() {
        return Err(UpdateError::NothingInstalled);
    }
    let source = Source::new(Some(&remembered_feed()?), None, false)?;
    let runtime = tokio::runtime::Runtime::new().map_err(UpdateError::Runtime)?;
    let Release::Feed(feed) = resolve(&runtime, &source, Carried::default())? else {
        return Err(RememberedFeedError::Local.into());
    };
    let version = feed.version().to_owned();
    let mut components: Vec<(App, InstalledVersion)> = Vec::new();
    for app in App::ALL {
        match installed(app)? {
            Installed::Present(current) => components.push((app, current)),
            Installed::Absent => {}
        }
    }
    let behind = |current: &InstalledVersion| match current {
        InstalledVersion::Known(current) => release::newer(&version, current),
        InstalledVersion::Missing | InstalledVersion::Unreadable { .. } => true,
    };
    let outdated = components.iter().any(|(_, current)| behind(current));
    let lines: Vec<String> = components
        .iter()
        .map(|(app, current)| {
            if behind(current) {
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
    let Some(placement) = place_ae(yes, false, None)? else {
        ui::outro("Nothing was updated.");
        return Ok(());
    };
    hand_over(
        &runtime,
        &feed,
        &EditorPlace::Keep,
        &ForgePlace::Keep,
        &Forwarded {
            yes,
            replace_ae: placement.agreed,
        },
    )?;
    ui::outro("Up to date.");
    Ok(())
}

fn removable(present: &[App], with_data: &[App], purge: bool) -> Vec<App> {
    App::ALL
        .into_iter()
        .filter(|app| present.contains(app) || (purge && with_data.contains(app)))
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
enum KeptData {
    Nothing,
    PurgeLater,
    DeleteByHand(Vec<PathBuf>),
}

fn kept_data(chosen: &[App], with_data: &[(App, PathBuf)], purge: bool) -> KeptData {
    let kept: Vec<PathBuf> = with_data
        .iter()
        .filter(|(app, _)| !purge && chosen.contains(app))
        .map(|(_, path)| path.clone())
        .collect();
    if kept.is_empty() {
        KeptData::Nothing
    } else if chosen.contains(&App::Ae) {
        KeptData::DeleteByHand(kept)
    } else {
        KeptData::PurgeLater
    }
}

fn say_uninstalled(kept: KeptData) {
    match kept {
        KeptData::Nothing => ui::outro("Uninstalled."),
        KeptData::PurgeLater => {
            ui::outro("Uninstalled. Data was kept; `ae uninstall --purge` removes it.");
        }
        KeptData::DeleteByHand(folders) => {
            let folders: Vec<String> = folders
                .iter()
                .map(|folder| folder.display().to_string())
                .collect();
            ui::note("Data was kept in", folders.join("\n"));
            ui::outro("Uninstalled. Delete those folders to remove the data too.");
        }
    }
}

fn leaves_orphans(chosen: &[App], present: &[App]) -> bool {
    chosen.contains(&App::Ae)
        && present
            .iter()
            .any(|app| *app != App::Ae && !chosen.contains(app))
}

fn recover_before_uninstalling() {
    if let Err(error) = recover_interrupted() {
        ui::warning(format!(
            "{}; uninstalling anyway",
            diagnostics::chain(&error)
        ));
    }
}

pub(crate) fn uninstall(apps: &[App], purge: bool, yes: bool) -> Result<(), UninstallError> {
    ui::intro("Artisan uninstall");
    recover_before_uninstalling();
    let mut present = Vec::new();
    let mut with_data = Vec::new();
    let mut hints = Vec::new();
    for app in App::ALL {
        let current = installed(app)?;
        let data = app
            .data()
            .map_err(|source| UninstallError::DataFolder { app, source })?;
        if data.try_exists().map_err(|source| UninstallError::Data {
            app,
            path: data.clone(),
            source,
        })? {
            with_data.push((app, data));
        }
        hints.push((
            app,
            match &current {
                Installed::Present(version) => version.to_string(),
                Installed::Absent => String::new(),
            },
        ));
        if current.is_present() {
            present.push(app);
        }
    }
    let has_data: Vec<App> = with_data.iter().map(|(app, _)| *app).collect();
    let candidates = removable(&present, &has_data, purge);
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
                let hint = hints
                    .iter()
                    .find_map(|(listed, hint)| (listed == app).then_some(hint.as_str()))
                    .expect("invariant: every app was checked before choosing");
                select = select.item(*app, app.display_name(), hint);
            }
            chosen = select
                .initial_values(candidates)
                .interact()
                .map_err(ui::prompt_error)?;
        }
        let keeping: Vec<&str> = chosen
            .iter()
            .filter(|app| has_data.contains(app))
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
    let kept = kept_data(&chosen, &with_data, purge);
    remove_chosen(chosen, purge)?;
    say_uninstalled(kept);
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
    let mut remaining = Vec::new();
    for app in App::ALL {
        if !chosen.contains(&app) && installed(app)?.is_present() {
            remaining.push(app);
        }
    }
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

struct SelfRemoval {
    ae: PathBuf,
    dir: PathBuf,
    lock: PathBuf,
    root: PathBuf,
    pointer: PathBuf,
    default: PathBuf,
}

fn self_removal_script(waiting_for: u32, removal: &SelfRemoval) -> String {
    format!(
        "$ErrorActionPreference = 'SilentlyContinue'; \
         Wait-Process -Id {waiting_for}; \
         function Remove-Empty($path) {{ \
           if ((Test-Path -LiteralPath $path -PathType Container) -and -not (Get-ChildItem -LiteralPath $path -Force)) {{ \
             Remove-Item -LiteralPath $path -Force; -not (Test-Path -LiteralPath $path) \
           }} else {{ $false }} \
         }}; \
         Remove-Item -LiteralPath {ae} -Force; \
         Remove-Empty {dir} | Out-Null; \
         Remove-Item -LiteralPath {lock} -Force; \
         if (Remove-Empty {root}) {{ Remove-Item -LiteralPath {pointer} -Force; Remove-Empty {default} | Out-Null }}",
        ae = sys::ps_quote(&removal.ae),
        dir = sys::ps_quote(&removal.dir),
        lock = sys::ps_quote(&removal.lock),
        root = sys::ps_quote(&removal.root),
        pointer = sys::ps_quote(&removal.pointer),
        default = sys::ps_quote(&removal.default),
    )
}

fn remove_self_later(ae: &Path, dir: &Path) -> Result<(), RemoveSelfLaterError> {
    let root = artisan_paths::root().map_err(RemoveSelfLaterError::Root)?;
    let lock = root.join(INSTALL_LOCK);
    let removal = SelfRemoval {
        ae: ae.to_owned(),
        dir: dir.to_owned(),
        lock,
        root,
        pointer: artisan_paths::root_pointer().map_err(RemoveSelfLaterError::Default)?,
        default: artisan_paths::default_root().map_err(RemoveSelfLaterError::Default)?,
    };
    sys::spawn_detached(&mut sys::powershell_command(&self_removal_script(
        std::process::id(),
        &removal,
    )))?;
    Ok(())
}

#[cfg(test)]
#[path = "../tests/install/plan.rs"]
mod tests;

#[cfg(all(test, unix))]
#[path = "../tests/install/foreign_ae_in_wsl.rs"]
mod foreign_ae_in_wsl_tests;
