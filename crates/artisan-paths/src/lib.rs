pub mod private;

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum App {
    Ae,
    Forge,
    Editor,
}

impl App {
    pub const ALL: [Self; 3] = [Self::Ae, Self::Forge, Self::Editor];

    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Ae => "ae",
            Self::Forge => "forge",
            Self::Editor => "editor",
        }
    }

    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Ae => "ae",
            Self::Forge => "Forge",
            Self::Editor => "Artisan Editor",
        }
    }

    #[must_use]
    pub fn binary_name(self) -> String {
        self.binary_name_on(std::env::consts::OS)
    }

    #[must_use]
    pub fn binary_name_on(self, os: &str) -> String {
        let windows = os == "windows";
        let stem = match self {
            Self::Ae => "ae",
            Self::Forge => "forge",
            Self::Editor if windows => "Artisan Editor",
            Self::Editor => "artisan-editor",
        };
        if windows {
            format!("{stem}.exe")
        } else {
            stem.to_owned()
        }
    }

    const fn folder(self) -> &'static str {
        match (self, cfg!(any(windows, target_os = "macos"))) {
            (Self::Ae, _) => "ae",
            (Self::Forge, true) => "Forge",
            (Self::Editor, true) => "Editor",
            (Self::Forge, false) => "forge",
            (Self::Editor, false) => "editor",
        }
    }

    pub fn dir(self) -> Result<PathBuf, RootError> {
        Ok(self.dir_under(&root()?))
    }

    #[must_use]
    pub fn dir_under(self, root: &Path) -> PathBuf {
        root.join(self.folder())
    }

    pub fn binary(self) -> Result<PathBuf, RootError> {
        Ok(self.dir()?.join(self.binary_name()))
    }

    pub fn data(self) -> Result<PathBuf, RootError> {
        Ok(self.dir()?.join("data"))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EnvPathError {
    #[error("{name} is not set")]
    Unset { name: &'static str },
    #[error("{name} is {}, which is not an absolute path", value.display())]
    Relative { name: &'static str, value: PathBuf },
}

#[derive(Debug, thiserror::Error)]
pub enum RootError {
    #[error("find the default Artisan Street folder")]
    Default(#[source] EnvPathError),
    #[error("read the location pointer {}", pointer.display())]
    ReadPointer {
        pointer: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("the location pointer {} is not UTF-8", pointer.display())]
    PointerNotUtf8 {
        pointer: PathBuf,
        #[source]
        source: std::string::FromUtf8Error,
    },
    #[error(
        "the location pointer {} names {}, which is not an absolute folder",
        pointer.display(),
        named.display()
    )]
    PointerNotAbsolute { pointer: PathBuf, named: PathBuf },
}

#[derive(Debug, thiserror::Error)]
pub enum ChooseRootError {
    #[error("find the default Artisan Street folder")]
    Default(#[source] EnvPathError),
    #[error("{} is not an absolute folder", chosen.display())]
    NotAbsolute { chosen: PathBuf },
    #[error("{} is not a UTF-8 path", chosen.display())]
    NotUtf8 { chosen: PathBuf },
    #[error("create the default Artisan Street folder {}", default.display())]
    CreateDefault {
        default: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("write the location pointer {}", pointer.display())]
    WritePointer {
        pointer: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("remove the location pointer {}", pointer.display())]
    RemovePointer {
        pointer: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("remove the empty default Artisan Street folder {}", default.display())]
    RemoveDefault {
        default: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

const LOCATION: &str = "location";
const LOCATION_DRAFT: &str = "location.new";

pub fn root() -> Result<PathBuf, RootError> {
    let default = default_root().map_err(RootError::Default)?;
    Ok(chosen_root(&default)?.unwrap_or(default))
}

pub fn default_root() -> Result<PathBuf, EnvPathError> {
    if cfg!(windows) {
        Ok(var("LOCALAPPDATA")?.join("Artisan Street"))
    } else if cfg!(target_os = "macos") {
        Ok(home()?.join("Library/Application Support/Artisan Street"))
    } else {
        Ok(xdg_data()?.join("artisan-street"))
    }
}

fn chosen_root(default: &Path) -> Result<Option<PathBuf>, RootError> {
    let pointer = default.join(LOCATION);
    let bytes = match std::fs::read(&pointer) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(RootError::ReadPointer { pointer, source }),
    };
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(source) => return Err(RootError::PointerNotUtf8 { pointer, source }),
    };
    let named = PathBuf::from(text.trim_end_matches(['\r', '\n']));
    if named.is_absolute() {
        Ok(Some(named))
    } else {
        Err(RootError::PointerNotAbsolute { pointer, named })
    }
}

fn point(default: &Path, chosen: &Path) -> Result<(), ChooseRootError> {
    let pointer = default.join(LOCATION);
    if chosen == default {
        return match std::fs::remove_file(&pointer) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(source) => Err(ChooseRootError::RemovePointer { pointer, source }),
        };
    }
    if !chosen.is_absolute() {
        return Err(ChooseRootError::NotAbsolute {
            chosen: chosen.to_owned(),
        });
    }
    let text = chosen.to_str().ok_or_else(|| ChooseRootError::NotUtf8 {
        chosen: chosen.to_owned(),
    })?;
    std::fs::create_dir_all(default).map_err(|source| ChooseRootError::CreateDefault {
        default: default.to_owned(),
        source,
    })?;
    let draft = default.join(LOCATION_DRAFT);
    std::fs::write(&draft, format!("{text}\n")).map_err(|source| {
        ChooseRootError::WritePointer {
            pointer: draft.clone(),
            source,
        }
    })?;
    std::fs::rename(&draft, &pointer)
        .map_err(|source| ChooseRootError::WritePointer { pointer, source })
}

fn forget(default: &Path) -> Result<(), ChooseRootError> {
    point(default, default)?;
    match std::fs::remove_dir(default) {
        Ok(()) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::NotFound | ErrorKind::DirectoryNotEmpty
            ) =>
        {
            Ok(())
        }
        Err(source) => Err(ChooseRootError::RemoveDefault {
            default: default.to_owned(),
            source,
        }),
    }
}

pub fn root_pointer() -> Result<PathBuf, EnvPathError> {
    Ok(default_root()?.join(LOCATION))
}

pub fn choose_root(chosen: &Path) -> Result<(), ChooseRootError> {
    point(&default_root().map_err(ChooseRootError::Default)?, chosen)
}

pub fn forget_root() -> Result<(), ChooseRootError> {
    forget(&default_root().map_err(ChooseRootError::Default)?)
}

pub fn home() -> Result<PathBuf, EnvPathError> {
    var(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
}

pub fn xdg_data() -> Result<PathBuf, EnvPathError> {
    optional_var("XDG_DATA_HOME")?.map_or_else(|| Ok(home()?.join(".local/share")), Ok)
}

pub fn xdg_config() -> Result<PathBuf, EnvPathError> {
    optional_var("XDG_CONFIG_HOME")?.map_or_else(|| Ok(home()?.join(".config")), Ok)
}

fn var(name: &'static str) -> Result<PathBuf, EnvPathError> {
    optional_var(name)?.ok_or(EnvPathError::Unset { name })
}

fn optional_var(name: &'static str) -> Result<Option<PathBuf>, EnvPathError> {
    absolute_var(name, std::env::var_os(name))
}

fn absolute_var(
    name: &'static str,
    value: Option<std::ffi::OsString>,
) -> Result<Option<PathBuf>, EnvPathError> {
    match value.filter(|value| !value.is_empty()).map(PathBuf::from) {
        None => Ok(None),
        Some(value) if value.is_absolute() => Ok(Some(value)),
        Some(value) => Err(EnvPathError::Relative { name, value }),
    }
}

#[cfg(test)]
#[path = "../tests/paths/location.rs"]
mod tests;
