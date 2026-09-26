//! Launch resolution and the explicit engine process environment.
//!
//! Every spawn of a managed engine (runs, model discovery, account usage,
//! version and auth probes) resolves through [`resolve_launch_target`]. The
//! only sources are the verified active generation below the registered Forge
//! state directory and, for development, one absolute-path override variable
//! per engine that is always reported as an override. `PATH`, npm shims,
//! `LOCALAPPDATA`, `WinGet`, and every other ambient location are never
//! consulted.

use std::{
    ffi::{OsStr, OsString},
    fmt,
    path::{Component, Path, PathBuf},
    sync::RwLock,
};

use crate::io as native_files;

use super::{
    authority::{ManagedEngineAuthority, ResolvedGeneration},
    catalog::ManagedEngine,
    spec::{EngineUseLease, ManagedInstallLockError},
    state::ManagedEngineError,
    version::EngineVersion,
};

static MANAGED_DATABASE: RwLock<Option<PathBuf>> = RwLock::new(None);

/// Registers the Forge database whose state directory owns the managed
/// engines. Called once by the Forge (and by `ae` for its instance) before
/// any engine is resolved; without it every engine is unavailable.
pub fn register_managed_database(database_path: &Path) {
    if let Ok(mut registered) = MANAGED_DATABASE.write() {
        *registered = Some(database_path.to_path_buf());
    }
}

/// Returns the registered Forge database, if any.
#[must_use]
pub fn managed_database() -> Option<PathBuf> {
    MANAGED_DATABASE.read().ok().and_then(|path| path.clone())
}

/// Where a launch target came from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LaunchSource {
    /// The verified active managed generation.
    Managed,
    /// The developer override variable; never used in production.
    Override,
}

impl LaunchSource {
    /// Returns the stable classification.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Managed => "managed",
            Self::Override => "override",
        }
    }
}

/// A resolved engine executable with everything needed to spawn it.
#[must_use = "retain the target for the protected spawn"]
pub struct LaunchTarget {
    engine: ManagedEngine,
    source: LaunchSource,
    executable: PathBuf,
    version: Option<EngineVersion>,
    tool_dirs: Vec<PathBuf>,
    database_path: PathBuf,
}

impl fmt::Debug for LaunchTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LaunchTarget")
            .field("engine", &self.engine)
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}

impl LaunchTarget {
    /// Returns the engine.
    #[must_use]
    pub const fn engine(&self) -> ManagedEngine {
        self.engine
    }

    /// Returns whether the target is managed or an override.
    #[must_use]
    pub const fn source(&self) -> LaunchSource {
        self.source
    }

    /// Returns the executable path.
    #[must_use]
    pub fn executable(&self) -> &Path {
        &self.executable
    }

    /// Returns the installed version for managed targets.
    #[must_use]
    pub const fn version(&self) -> Option<&EngineVersion> {
        self.version.as_ref()
    }

    /// Returns the engine's private `HOME` below the Forge state directory.
    #[must_use]
    pub fn home(&self) -> PathBuf {
        engine_home(
            self.database_path.parent().unwrap_or(Path::new("/")),
            self.engine,
        )
    }

    /// Builds the complete child environment from the live Forge process.
    ///
    /// # Errors
    ///
    /// Returns [`ManagedEngineError`] when the private engine home cannot be
    /// created with private permissions.
    pub fn environment(&self) -> Result<Vec<(OsString, OsString)>, ManagedEngineError> {
        let home = self.home();
        prepare_home(&home)?;
        Ok(build_environment(
            self.engine,
            &home,
            &self.tool_dirs,
            &|name| std::env::var_os(name),
        ))
    }

    /// Takes the shared use lease that defers generation switches while the
    /// engine process runs. Overrides are not managed and need no lease.
    ///
    /// # Errors
    ///
    /// Returns [`ManagedInstallLockError`] when the lease cannot be taken.
    pub fn lease(&self) -> Result<Option<EngineUseLease>, ManagedInstallLockError> {
        if self.source == LaunchSource::Override {
            return Ok(None);
        }
        let paths = ManagedEngineAuthority::new(self.engine)
            .install_paths(&self.database_path)
            .map_err(|_| ManagedInstallLockError::InvalidRoot)?;
        EngineUseLease::acquire(&paths).map(Some)
    }
}

/// A launch target seated for one process: the executable, its complete
/// environment, and the use lease that must live as long as the process.
#[must_use = "retain the seated launch for as long as the engine process runs"]
pub struct SeatedLaunch {
    executable: PathBuf,
    environment: Vec<(OsString, OsString)>,
    source: LaunchSource,
    _lease: Option<EngineUseLease>,
}

impl fmt::Debug for SeatedLaunch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SeatedLaunch")
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}

impl SeatedLaunch {
    /// Resolves and seats `engine` for the Forge database.
    ///
    /// # Errors
    ///
    /// Returns [`ManagedEngineError`] when the engine cannot be resolved, its
    /// home cannot be prepared, or the use lease cannot be taken.
    pub fn seat(engine: ManagedEngine, database_path: &Path) -> Result<Self, ManagedEngineError> {
        let target =
            resolve_launch_target_in(engine, database_path, &|name| std::env::var_os(name))?;
        Self::from_target(&target)
    }

    /// Seats an already resolved target.
    ///
    /// # Errors
    ///
    /// Returns [`ManagedEngineError`] when the home or lease is unavailable.
    pub fn from_target(target: &LaunchTarget) -> Result<Self, ManagedEngineError> {
        Ok(Self {
            executable: target.executable().to_path_buf(),
            environment: target.environment()?,
            source: target.source(),
            _lease: target.lease().map_err(|_| ManagedEngineError::Io)?,
        })
    }

    /// Fixture seam for owner tests: seats an explicit program with the
    /// managed environment of `database_path`'s engine home. Compiled only
    /// with the `fixtures` feature, never into production builds.
    ///
    /// # Errors
    ///
    /// Returns [`ManagedEngineError`] when the engine home is unavailable.
    #[cfg(feature = "fixtures")]
    pub fn explicit(
        engine: ManagedEngine,
        database_path: &Path,
        executable: &Path,
    ) -> Result<Self, ManagedEngineError> {
        let target = LaunchTarget {
            engine,
            source: LaunchSource::Override,
            executable: executable.to_path_buf(),
            version: None,
            tool_dirs: Vec::new(),
            database_path: database_path.to_path_buf(),
        };
        Self::from_target(&target)
    }

    /// Test seam: a fixture program with an explicit environment.
    #[cfg(test)]
    pub(crate) fn fixture(executable: PathBuf) -> Self {
        Self {
            executable,
            environment: Vec::new(),
            source: LaunchSource::Override,
            _lease: None,
        }
    }

    /// Returns the executable path.
    #[must_use]
    pub fn executable(&self) -> &Path {
        &self.executable
    }

    /// Returns the complete child environment; spawn with `env_clear`.
    #[must_use]
    pub fn environment(&self) -> &[(OsString, OsString)] {
        &self.environment
    }

    /// Returns whether the executable is managed or an override.
    #[must_use]
    pub const fn source(&self) -> LaunchSource {
        self.source
    }
}

/// Resolves `engine` for the registered Forge state directory.
///
/// # Errors
///
/// Returns [`ManagedEngineError::StateMissing`] when no Forge database is
/// registered or the engine is not installed, and the authority's error when
/// the active generation fails verification.
pub fn resolve_launch_target(engine: ManagedEngine) -> Result<LaunchTarget, ManagedEngineError> {
    let database = managed_database().ok_or(ManagedEngineError::StateMissing)?;
    resolve_launch_target_in(engine, &database, &|name| std::env::var_os(name))
}

/// Resolves `engine` for an explicit Forge database and override lookup.
///
/// # Errors
///
/// Returns [`ManagedEngineError`] when neither a valid override nor a
/// verified managed generation exists.
pub fn resolve_launch_target_in(
    engine: ManagedEngine,
    database_path: &Path,
    variable: &dyn Fn(&str) -> Option<OsString>,
) -> Result<LaunchTarget, ManagedEngineError> {
    if !database_path.parent().is_some_and(Path::is_absolute) {
        return Err(ManagedEngineError::UnsafePath);
    }
    if let Some(executable) = override_executable(engine, variable)? {
        return Ok(LaunchTarget {
            engine,
            source: LaunchSource::Override,
            executable,
            version: None,
            tool_dirs: Vec::new(),
            database_path: database_path.to_path_buf(),
        });
    }
    let generation: ResolvedGeneration =
        ManagedEngineAuthority::new(engine).resolve_active(database_path)?;
    Ok(LaunchTarget {
        engine,
        source: LaunchSource::Managed,
        executable: generation.executable_path().to_path_buf(),
        version: Some(generation.version().clone()),
        tool_dirs: generation.tool_dirs().to_vec(),
        database_path: database_path.to_path_buf(),
    })
}

/// Reads the developer override: an absolute path to an existing regular
/// file. Anything else (relative names that would need `PATH`, links,
/// directories) is rejected rather than ignored.
fn override_executable(
    engine: ManagedEngine,
    variable: &dyn Fn(&str) -> Option<OsString>,
) -> Result<Option<PathBuf>, ManagedEngineError> {
    let Some(value) = variable(engine.override_env()) else {
        return Ok(None);
    };
    let text = value.to_string_lossy();
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let path = PathBuf::from(trimmed);
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(ManagedEngineError::UnsafePath);
    }
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => Ok(Some(path)),
        Ok(_) => Err(ManagedEngineError::UnsafePath),
        Err(_) => Err(ManagedEngineError::ExecutableUnavailable),
    }
}

/// Returns `<state>/toolchain/<engine>/home`.
#[must_use]
pub fn engine_home(state_root: &Path, engine: ManagedEngine) -> PathBuf {
    state_root.join("toolchain").join(engine.id()).join("home")
}

/// Creates `<state>/toolchain/<engine>/home` (private) and its two managed
/// ancestors; the Forge state directory itself must already exist.
fn prepare_home(home: &Path) -> Result<(), ManagedEngineError> {
    let engine_root = home.parent().ok_or(ManagedEngineError::UnsafePath)?;
    let toolchain = engine_root.parent().ok_or(ManagedEngineError::UnsafePath)?;
    for directory in [toolchain, engine_root] {
        native_files::ensure_directory(directory).map_err(|_| ManagedEngineError::Io)?;
    }
    native_files::ensure_private_directory(home).map_err(|_| ManagedEngineError::UnsafePath)
}

/// Variables copied from the Forge when present. Nothing else crosses.
const PASS_THROUGH: &[&str] = &[
    "TERM",
    "TZ",
    "TMPDIR",
    "HTTPS_PROXY",
    "HTTP_PROXY",
    "NO_PROXY",
    "https_proxy",
    "http_proxy",
    "no_proxy",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "NODE_EXTRA_CA_CERTS",
    "SYSTEMROOT",
    "SystemRoot",
    "WINDIR",
    "COMSPEC",
    "PATHEXT",
    "TEMP",
    "TMP",
    "PROGRAMDATA",
    "PROGRAMFILES",
    "NUMBER_OF_PROCESSORS",
    "PROCESSOR_ARCHITECTURE",
];

/// Credentials an operator may deliberately provide to the Forge.
const fn credential_variables(engine: ManagedEngine) -> &'static [&'static str] {
    match engine {
        ManagedEngine::Claude => &["ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN"],
        ManagedEngine::Codex => &["OPENAI_API_KEY", "CODEX_API_KEY"],
        ManagedEngine::Grok => &["XAI_API_KEY"],
        ManagedEngine::Cursor => &["CURSOR_API_KEY"],
        ManagedEngine::OpenCode2 => &[],
    }
}

/// Builds the complete engine environment. Pure over `variable` so tests
/// supply a fixture host environment.
#[must_use]
pub fn build_environment(
    engine: ManagedEngine,
    home: &Path,
    tool_dirs: &[PathBuf],
    variable: &dyn Fn(&str) -> Option<OsString>,
) -> Vec<(OsString, OsString)> {
    let mut environment: Vec<(OsString, OsString)> = Vec::new();
    let mut set = |name: &str, value: OsString| {
        environment.retain(|(existing, _)| existing != name);
        environment.push((OsString::from(name), value));
    };
    for name in PASS_THROUGH
        .iter()
        .chain(credential_variables(engine).iter())
    {
        if let Some(value) = variable(name).filter(|value| !value.is_empty()) {
            set(name, value);
        }
    }
    set("HOME", home.as_os_str().to_owned());
    if cfg!(windows) {
        set("USERPROFILE", home.as_os_str().to_owned());
        set(
            "APPDATA",
            home.join("AppData").join("Roaming").into_os_string(),
        );
        set(
            "LOCALAPPDATA",
            home.join("AppData").join("Local").into_os_string(),
        );
    }
    match engine {
        ManagedEngine::Claude => {
            set("CLAUDE_CONFIG_DIR", home.join(".claude").into_os_string());
            set("DISABLE_UPDATES", OsString::from("1"));
        }
        ManagedEngine::Codex => {
            set("CODEX_HOME", home.join(".codex").into_os_string());
        }
        ManagedEngine::Grok | ManagedEngine::Cursor | ManagedEngine::OpenCode2 => {}
    }
    let locale = variable("LANG")
        .filter(|value| {
            let value = value.to_string_lossy().to_ascii_lowercase();
            value.contains("utf-8") || value.contains("utf8")
        })
        .unwrap_or_else(|| OsString::from("C.UTF-8"));
    set("LANG", locale.clone());
    set("LC_ALL", locale);
    set("PATH", engine_path(tool_dirs, variable("PATH").as_deref()));
    environment
}

/// The engine `PATH`: the generation's own tool directories, then the
/// Forge's absolute `PATH` entries. On Linux, Windows interop mounts
/// (`/mnt/<drive>/…`) are removed so a WSL Forge never reaches Windows
/// binaries or shims. Falls back to the standard system directories.
fn engine_path(tool_dirs: &[PathBuf], forge_path: Option<&OsStr>) -> OsString {
    let inherited = forge_path
        .map(|paths| std::env::split_paths(paths).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .filter(|entry| entry.is_absolute() && !is_foreign_mount(entry))
        .collect::<Vec<_>>();
    let fallback = if cfg!(windows) {
        Vec::new()
    } else {
        ["/usr/local/bin", "/usr/bin", "/bin"]
            .into_iter()
            .map(PathBuf::from)
            .collect()
    };
    let system = if inherited.is_empty() {
        fallback
    } else {
        inherited
    };
    let mut entries: Vec<PathBuf> = Vec::new();
    for entry in tool_dirs.iter().cloned().chain(system) {
        if !entries.contains(&entry) {
            entries.push(entry);
        }
    }
    std::env::join_paths(entries).unwrap_or_default()
}

fn is_foreign_mount(entry: &Path) -> bool {
    if !cfg!(target_os = "linux") {
        return false;
    }
    let mut components = entry.components();
    matches!(
        (components.next(), components.next(), components.next()),
        (Some(Component::RootDir), Some(Component::Normal(mnt)), Some(Component::Normal(drive)))
            if mnt == "mnt" && drive.len() == 1
    )
}

#[cfg(test)]
#[path = "launch_tests.rs"]
mod tests;
