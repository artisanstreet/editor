//! The per-user Forge service of an installation: a systemd user unit.
//!
//! Linux support is systemd-only. `ae setup --autostart` renders a unit that
//! runs the installation's permanent launcher, `<root>/bin/ae start
//! --foreground`, which supervises the active version's Forge and publishes
//! its host invitation. The unit is owned by the installation: it carries an
//! `X-ArtisanInstallRoot=` marker naming the root, Artisan rewrites it
//! whenever it configures the service, refuses to touch a unit of the same
//! name that it did not write, and `ae autostart --disable` removes it.
//!
//! The Forge only shuts down gracefully on SIGINT, so the unit stops with
//! SIGINT; a Forge killed anyway leaves a readiness receipt behind, which the
//! supervisor reconciles on the next start.

use std::{
    ffi::OsStr,
    fmt::Write as _,
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use sha2::{Digest as _, Sha256};

use crate::{CliError, Result};

/// Unit key naming the installation root that owns a unit.
pub const OWNER_KEY: &str = "X-ArtisanInstallRoot";

/// Name prefix of every Artisan Forge unit.
const UNIT_PREFIX: &str = "artisan-forge";

/// Root directory name of the default installation; channel installations
/// append a suffix (`Artisan Street Dev`).
const ROOT_NAME: &str = "Artisan Street";

/// The outcome of one `systemctl --user` invocation.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SystemctlOutput {
    /// Whether the command exited successfully.
    pub success: bool,
    /// Trimmed standard output.
    pub stdout: String,
}

/// The seam over `systemctl --user`, so unit management is testable without
/// a user manager.
pub trait Systemctl {
    /// Runs `systemctl --user` with `arguments`.
    ///
    /// # Errors
    ///
    /// Returns [`CliError::Service`] when `systemctl` cannot be run at all.
    fn run(&self, arguments: &[&str]) -> Result<SystemctlOutput>;
}

/// The real per-user service manager.
#[derive(Clone, Copy, Debug, Default)]
pub struct UserSystemctl;

impl Systemctl for UserSystemctl {
    fn run(&self, arguments: &[&str]) -> Result<SystemctlOutput> {
        let output = Command::new("systemctl")
            .arg("--user")
            .args(arguments)
            .stdin(Stdio::null())
            .stderr(Stdio::inherit())
            .output()
            .map_err(|error| {
                CliError::Service(format!(
                    "cannot run `systemctl --user` ({error}); Linux installations need a systemd user manager"
                ))
            })?;
        Ok(SystemctlOutput {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        })
    }
}

/// The user directories units and installations live in.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UserDirectories {
    /// `$HOME`.
    pub home: Option<PathBuf>,
    /// `$XDG_CONFIG_HOME`.
    pub config_home: Option<PathBuf>,
    /// `$XDG_DATA_HOME`.
    pub data_home: Option<PathBuf>,
}

impl UserDirectories {
    /// Reads the directories from the process environment.
    #[must_use]
    pub fn from_env() -> Self {
        let variable = |name: &str| {
            std::env::var_os(name)
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
        };
        Self {
            home: variable("HOME"),
            config_home: variable("XDG_CONFIG_HOME"),
            data_home: variable("XDG_DATA_HOME"),
        }
    }

    /// `$XDG_CONFIG_HOME/systemd/user`, defaulting to `~/.config`.
    ///
    /// # Errors
    ///
    /// Returns [`CliError::Service`] without a usable home directory.
    pub fn unit_directory(&self) -> Result<PathBuf> {
        self.config_home
            .clone()
            .or_else(|| self.home.as_ref().map(|home| home.join(".config")))
            .map(|config| config.join("systemd").join("user"))
            .ok_or_else(|| CliError::Service("no home directory for user units".to_owned()))
    }

    /// `$XDG_DATA_HOME`, defaulting to `~/.local/share`.
    #[must_use]
    pub fn data_directory(&self) -> Option<PathBuf> {
        self.data_home.clone().or_else(|| {
            self.home
                .as_ref()
                .map(|home| home.join(".local").join("share"))
        })
    }
}

/// Unit name for the installation at `root`.
///
/// Installations in the user data directory named `Artisan Street` or
/// `Artisan Street <Channel>` get a readable name (`artisan-forge`,
/// `artisan-forge-dev`); any other root gets a name derived from its path,
/// so side-by-side installations never share a unit.
#[must_use]
pub fn unit_name_for(root: &Path, data_directory: Option<&Path>) -> String {
    let readable = root
        .file_name()
        .and_then(OsStr::to_str)
        .and_then(|name| name.strip_prefix(ROOT_NAME))
        .filter(|_| root.parent() == data_directory)
        .and_then(|suffix| {
            let slug = slug(suffix);
            match (suffix.is_empty(), slug.is_empty()) {
                (true, _) => Some(format!("{UNIT_PREFIX}.service")),
                (false, false) if suffix.starts_with(' ') => {
                    Some(format!("{UNIT_PREFIX}-{slug}.service"))
                }
                (false, _) => None,
            }
        });
    readable.unwrap_or_else(|| {
        let digest = Sha256::digest(root.as_os_str().as_encoded_bytes());
        let hex = digest.iter().take(6).fold(String::new(), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        });
        format!("{UNIT_PREFIX}-{hex}.service")
    })
}

fn slug(text: &str) -> String {
    let mut slug = String::new();
    for character in text.trim().chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.trim_matches('-').to_owned()
}

/// Escapes systemd specifiers (`%`) and C escapes for a quoted value.
fn escape_value(text: &str) -> Result<String> {
    if text.chars().any(char::is_control) {
        return Err(CliError::Service(
            "unit values cannot contain control characters".to_owned(),
        ));
    }
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '%' => escaped.push_str("%%"),
            character => escaped.push(character),
        }
    }
    Ok(escaped)
}

/// Quotes one `ExecStart=` argument: a quoted word whose `$` is literal.
///
/// # Errors
///
/// Returns [`CliError::Service`] for text with control characters.
pub fn quote_exec_argument(text: &str) -> Result<String> {
    Ok(format!("\"{}\"", escape_value(text)?.replace('$', "$$")))
}

/// How the unit file of an installation currently stands.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnitFile {
    /// No unit of this name exists.
    Absent,
    /// This installation's unit; `current` when it launches this
    /// installation's permanent `ae` exactly as rendered.
    Owned {
        /// Whether its `ExecStart=` matches the rendered unit.
        current: bool,
    },
    /// A unit of this name that this installation did not write.
    Foreign,
}

/// What `ae doctor` reports about an installation's service.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ServiceHealth {
    /// No unit: the Forge is not autostarted (`ae setup --autostart`).
    Absent,
    /// A unit of this name that this installation did not write.
    Foreign,
    /// This installation's unit, no longer launching its permanent `ae`.
    Drifted,
    /// This installation's current unit; `None` when the manager could not
    /// be asked.
    Installed {
        /// Whether it starts at login.
        enabled: Option<bool>,
        /// Whether it runs now.
        active: Option<bool>,
    },
    /// The unit could not be inspected.
    Unavailable(String),
}

impl ServiceHealth {
    /// Stable state name: `ok`, `absent`, `drifted`, `foreign`, or
    /// `unavailable`.
    #[must_use]
    pub const fn state(&self) -> &'static str {
        match self {
            Self::Absent => "absent",
            Self::Foreign => "foreign",
            Self::Drifted => "drifted",
            Self::Installed { .. } => "ok",
            Self::Unavailable(_) => "unavailable",
        }
    }

    /// Whether the installation is healthy: an absent service is only a
    /// choice not to autostart.
    #[must_use]
    pub const fn is_healthy(&self) -> bool {
        matches!(self, Self::Absent | Self::Installed { .. })
    }
}

/// The Forge service of one installation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ForgeService {
    root: PathBuf,
    unit_name: String,
    unit_path: PathBuf,
}

impl ForgeService {
    /// The service of the installation at `root`.
    ///
    /// # Errors
    ///
    /// Returns [`CliError::Service`] without a usable unit directory.
    pub fn for_root(root: &Path, directories: &UserDirectories) -> Result<Self> {
        let unit_name = unit_name_for(root, directories.data_directory().as_deref());
        Ok(Self {
            root: root.to_path_buf(),
            unit_path: directories.unit_directory()?.join(&unit_name),
            unit_name,
        })
    }

    /// The service of the installation at `root`, for this user.
    ///
    /// # Errors
    ///
    /// Returns [`CliError::Service`] without a usable unit directory.
    pub fn for_current_user(root: &Path) -> Result<Self> {
        Self::for_root(root, &UserDirectories::from_env())
    }

    /// `artisan-forge….service`.
    #[must_use]
    pub fn unit_name(&self) -> &str {
        &self.unit_name
    }

    /// Where the unit file lives.
    #[must_use]
    pub fn unit_path(&self) -> &Path {
        &self.unit_path
    }

    fn permanent_ae(&self) -> PathBuf {
        self.root.join("bin").join("ae")
    }

    fn exec_start(&self) -> Result<String> {
        Ok(format!(
            "ExecStart={} start --foreground",
            quote_exec_argument(&self.permanent_ae().to_string_lossy())?
        ))
    }

    fn owner_line(&self) -> Result<String> {
        Ok(format!(
            "{OWNER_KEY}={}",
            escape_value(&self.root.to_string_lossy())?
        ))
    }

    /// Renders the unit. `path` becomes the service's `PATH`, so agent tools
    /// the Forge runs resolve as they do in the configuring user's shell.
    ///
    /// # Errors
    ///
    /// Returns [`CliError::Service`] for paths with control characters.
    pub fn render(&self, path: Option<&str>) -> Result<String> {
        let name = self
            .root
            .file_name()
            .map_or_else(|| self.root.to_string_lossy(), OsStr::to_string_lossy);
        let environment = match path {
            Some(path) => format!("Environment=\"PATH={}\"\n", escape_value(path)?),
            None => String::new(),
        };
        Ok(format!(
            "# Artisan Forge service of the installation at {root}.\n\
             # Written by `ae setup --autostart` and removed by `ae autostart --disable`;\n\
             # Artisan replaces local edits whenever it configures the service.\n\
             [Unit]\n\
             Description=Artisan Forge ({name})\n\
             {owner}\n\
             After=network-online.target\n\
             \n\
             [Service]\n\
             Type=simple\n\
             {exec_start}\n\
             {environment}\
             Restart=on-failure\n\
             RestartSec=5\n\
             KillSignal=SIGINT\n\
             TimeoutStopSec=30\n\
             UMask=0077\n\
             \n\
             [Install]\n\
             WantedBy=default.target\n",
            root = escape_value(&self.root.to_string_lossy())?,
            name = escape_value(&name)?,
            owner = self.owner_line()?,
            exec_start = self.exec_start()?,
        ))
    }

    /// Inspects the unit file without changing anything.
    ///
    /// # Errors
    ///
    /// Returns [`CliError::Service`] when an existing unit cannot be read.
    pub fn inspect(&self) -> Result<UnitFile> {
        let text = match fs::read_to_string(&self.unit_path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(UnitFile::Absent);
            }
            Err(error) => {
                return Err(CliError::Service(format!(
                    "cannot read {}: {error}",
                    self.unit_path.display()
                )));
            }
        };
        let owner = self.owner_line()?;
        if !text.lines().any(|line| line.trim() == owner) {
            return Ok(UnitFile::Foreign);
        }
        let exec_start = self.exec_start()?;
        Ok(UnitFile::Owned {
            current: text.lines().any(|line| line.trim() == exec_start),
        })
    }

    /// Writes the unit when it differs, reloads the manager, and enables the
    /// service at login. Returns whether the unit file changed.
    ///
    /// # Errors
    ///
    /// Returns [`CliError::Service`] for a foreign unit of the same name or
    /// when the manager refuses.
    pub fn install(&self, systemctl: &dyn Systemctl, path: Option<&str>) -> Result<bool> {
        if self.inspect()? == UnitFile::Foreign {
            return Err(CliError::Service(format!(
                "{} exists but was not written for this installation; move it aside first",
                self.unit_path.display()
            )));
        }
        let rendered = self.render(path)?;
        let changed = fs::read_to_string(&self.unit_path).ok().as_deref() != Some(&rendered);
        if changed {
            write_atomically(&self.unit_path, rendered.as_bytes())?;
        }
        require(systemctl, &["daemon-reload"])?;
        require(systemctl, &["enable", &self.unit_name])?;
        Ok(changed)
    }

    /// Stops and disables the service and removes its unit.
    ///
    /// # Errors
    ///
    /// Returns [`CliError::Service`] for a foreign unit or a manager failure.
    pub fn remove(&self, systemctl: &dyn Systemctl) -> Result<()> {
        match self.inspect()? {
            UnitFile::Absent => Ok(()),
            UnitFile::Foreign => Err(CliError::Service(format!(
                "{} was not written for this installation; leaving it alone",
                self.unit_path.display()
            ))),
            UnitFile::Owned { .. } => {
                require(systemctl, &["disable", "--now", &self.unit_name])?;
                fs::remove_file(&self.unit_path).map_err(|error| {
                    CliError::Service(format!(
                        "cannot remove {}: {error}",
                        self.unit_path.display()
                    ))
                })?;
                require(systemctl, &["daemon-reload"])
            }
        }
    }

    /// The service's health, asking `systemctl` only about a current unit.
    #[must_use]
    pub fn health(&self, systemctl: &dyn Systemctl) -> ServiceHealth {
        match self.inspect() {
            Ok(UnitFile::Absent) => ServiceHealth::Absent,
            Ok(UnitFile::Foreign) => ServiceHealth::Foreign,
            Ok(UnitFile::Owned { current: false }) => ServiceHealth::Drifted,
            Ok(UnitFile::Owned { current: true }) => ServiceHealth::Installed {
                enabled: self.is_enabled(systemctl).ok(),
                active: self.is_active(systemctl).ok(),
            },
            Err(error) => ServiceHealth::Unavailable(error.to_string()),
        }
    }

    /// Whether the service starts at login.
    ///
    /// # Errors
    ///
    /// Returns [`CliError::Service`] when `systemctl` cannot run.
    pub fn is_enabled(&self, systemctl: &dyn Systemctl) -> Result<bool> {
        Ok(systemctl.run(&["is-enabled", &self.unit_name])?.stdout == "enabled")
    }

    /// Whether the service is running.
    ///
    /// # Errors
    ///
    /// Returns [`CliError::Service`] when `systemctl` cannot run.
    pub fn is_active(&self, systemctl: &dyn Systemctl) -> Result<bool> {
        Ok(systemctl.run(&["is-active", &self.unit_name])?.stdout == "active")
    }

    /// Starts the service (a no-op while it runs).
    ///
    /// # Errors
    ///
    /// Returns [`CliError::Service`] when the manager refuses.
    pub fn start(&self, systemctl: &dyn Systemctl) -> Result<()> {
        require(systemctl, &["start", &self.unit_name])
    }

    /// Restarts the service only if it is running, applying a changed
    /// configuration.
    ///
    /// # Errors
    ///
    /// Returns [`CliError::Service`] when the manager refuses.
    pub fn try_restart(&self, systemctl: &dyn Systemctl) -> Result<()> {
        require(systemctl, &["try-restart", &self.unit_name])
    }
}

fn require(systemctl: &dyn Systemctl, arguments: &[&str]) -> Result<()> {
    if systemctl.run(arguments)?.success {
        Ok(())
    } else {
        Err(CliError::Service(format!(
            "`systemctl --user {}` failed",
            arguments.join(" ")
        )))
    }
}

fn write_atomically(path: &Path, bytes: &[u8]) -> Result<()> {
    let failed = |error: std::io::Error| {
        CliError::Service(format!("cannot write {}: {error}", path.display()))
    };
    let directory = path
        .parent()
        .ok_or_else(|| CliError::Service(format!("{} has no parent", path.display())))?;
    fs::create_dir_all(directory).map_err(failed)?;
    let temporary = directory.join(format!(
        ".{}.tmp",
        path.file_name()
            .map_or_else(Default::default, OsStr::to_string_lossy)
    ));
    let mut file = fs::File::create(&temporary).map_err(failed)?;
    file.write_all(bytes).map_err(failed)?;
    file.sync_all().map_err(failed)?;
    drop(file);
    fs::rename(&temporary, path).map_err(failed)
}
