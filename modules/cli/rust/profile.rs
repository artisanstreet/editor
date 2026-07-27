use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

use crate::{error::io, paths::Layout, CliError, Result};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Profile {
    pub data_root: PathBuf,
    pub listen_host: String,
    pub listen_port: u16,
    pub mode: ForgeMode,
    pub version: u8,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ForgeMode {
    Local,
    Headless,
}

#[derive(Clone, Debug, Deserialize)]
pub struct State {
    pub endpoint: String,
    pub instance_id: String,
    pub pid: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Secrets {
    pub auth_token: String,
    pub version: u8,
}

#[derive(Clone, Debug)]
pub struct ProfilePaths {
    pub directory: PathBuf,
    pub config: PathBuf,
    pub secrets: PathBuf,
    pub state: PathBuf,
    pub log: PathBuf,
}

pub fn validate_name(name: &str) -> Result<()> {
    let valid = !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .enumerate()
            .all(|(index, byte)| byte.is_ascii_alphanumeric() || (index > 0 && matches!(byte, b'_' | b'-')));
    let reserved = matches!(
        name.to_ascii_uppercase().as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "COM1" | "COM2" | "COM3" | "COM4" | "COM5"
            | "COM6" | "COM7" | "COM8" | "COM9" | "LPT1" | "LPT2" | "LPT3" | "LPT4"
            | "LPT5" | "LPT6" | "LPT7" | "LPT8" | "LPT9"
    );
    if valid && !reserved {
        Ok(())
    } else {
        Err(CliError::InvalidProfile(name.into()))
    }
}

pub fn paths(layout: &Layout, name: &str) -> Result<ProfilePaths> {
    validate_name(name)?;
    let directory = layout.profiles.join(name);
    Ok(ProfilePaths {
        config: directory.join("config.json"),
        secrets: directory.join("secrets.json"),
        state: directory.join("state.json"),
        log: directory.join("forge.log"),
        directory,
    })
}

pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = fs::read(path).map_err(io("read profile file"))?;
    serde_json::from_slice(&bytes).map_err(|source| CliError::Json {
        path: path.to_path_buf(),
        source,
    })
}

pub fn load(layout: &Layout, name: &str) -> Result<(ProfilePaths, Profile, Secrets)> {
    let paths = paths(layout, name)?;
    if !paths.config.is_file() || !paths.secrets.is_file() {
        return Err(CliError::MissingProfile(name.into()));
    }
    let profile = read_json(&paths.config)?;
    let secrets = read_json(&paths.secrets)?;
    Ok((paths, profile, secrets))
}

pub fn list_names(layout: &Layout) -> Result<Vec<String>> {
    let entries = match fs::read_dir(&layout.profiles) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(CliError::Io {
                context: "enumerate Forge profiles",
                source,
            });
        }
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(io("enumerate Forge profile"))?;
        let file_type = entry
            .file_type()
            .map_err(io("inspect Forge profile directory"))?;
        if !file_type.is_dir() || file_type.is_symlink() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if validate_name(&name).is_ok() {
            names.push(name);
        }
    }
    names.sort_unstable();
    Ok(names)
}

pub fn setup(
    layout: &Layout,
    name: &str,
    mode: ForgeMode,
    port: u16,
    data_root: Option<&Path>,
) -> Result<()> {
    let paths = paths(layout, name)?;
    fs::create_dir_all(&paths.directory).map_err(io("create profile directory"))?;
    restrict_directory(&paths.directory)?;
    let profile = Profile {
        data_root: data_root.map_or_else(|| paths.directory.join("data"), Path::to_path_buf),
        listen_host: "127.0.0.1".into(),
        listen_port: port,
        mode,
        version: 1,
    };
    write_private_json(&paths.config, &profile)?;
    if !paths.secrets.exists() {
        let mut token = [0_u8; 32];
        getrandom::fill(&mut token)
            .map_err(|error| CliError::Installation(format!("secure random source failed: {error}")))?;
        write_private_json(
            &paths.secrets,
            &Secrets {
                auth_token: URL_SAFE_NO_PAD.encode(token),
                version: 1,
            },
        )?;
    }
    Ok(())
}

fn write_private_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|source| CliError::Json {
        path: path.to_path_buf(),
        source,
    })?;
    reject_unsafe_destination(path)?;
    let directory = path
        .parent()
        .ok_or_else(|| CliError::UnsafePath(path.to_path_buf()))?;
    let mut nonce = [0_u8; 16];
    getrandom::fill(&mut nonce)
        .map_err(|error| CliError::Installation(format!("secure random source failed: {error}")))?;
    let temporary = directory.join(format!(
        ".{}.{}.tmp",
        path.file_name().and_then(|name| name.to_str()).unwrap_or("profile"),
        URL_SAFE_NO_PAD.encode(nonce)
    ));
    let result = (|| -> Result<()> {
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .map_err(io("create temporary profile file"))?;
        file.write_all(&bytes)
            .map_err(io("write temporary profile file"))?;
        file.sync_all()
            .map_err(io("sync temporary profile file"))?;
        drop(file);
        reject_unsafe_destination(path)?;
        fs::rename(&temporary, path).map_err(io("activate profile file"))?;
        sync_directory(directory)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn reject_unsafe_destination(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(CliError::UnsafePath(path.to_path_buf()))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(CliError::Io {
            context: "inspect profile destination",
            source,
        }),
    }
}

#[cfg(unix)]
fn restrict_directory(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(io("restrict profile directory"))
}

#[cfg(not(unix))]
fn restrict_directory(path: &Path) -> Result<()> {
    // The installer places ARTISAN_HOME in a current-user directory. Rust's
    // safe standard library has no Windows ACL API; do not broaden its ACL.
    fs::metadata(path)
        .map(|_| ())
        .map_err(io("inspect profile directory"))
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(io("sync profile directory"))
}

#[cfg(not(unix))]
fn sync_directory(path: &Path) -> Result<()> {
    fs::metadata(path)
        .map(|_| ())
        .map_err(io("inspect profile directory"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_single_component_profile_names() {
        assert!(validate_name("default").is_ok());
        assert!(validate_name("team-1").is_ok());
        assert!(validate_name("../escape").is_err());
        assert!(validate_name("CON").is_err());
    }

    #[test]
    fn private_writer_replaces_regular_files_without_leaving_temporary_files() {
        let directory = std::env::temp_dir().join(format!(
            "artisan-cli-profile-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir(&directory).unwrap();
        let destination = directory.join("config.json");
        write_private_json(&destination, &serde_json::json!({ "version": 1 })).unwrap();
        write_private_json(&destination, &serde_json::json!({ "version": 2 })).unwrap();
        let value: serde_json::Value = read_json(&destination).unwrap();
        assert_eq!(value["version"], 2);
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn profile_enumeration_returns_only_valid_directories() {
        let root =
            std::env::temp_dir().join(format!("artisan-cli-list-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let profiles = root.join("profiles");
        fs::create_dir_all(profiles.join("default")).unwrap();
        fs::create_dir(profiles.join("team-1")).unwrap();
        fs::create_dir(profiles.join("invalid.name")).unwrap();
        fs::write(profiles.join("not-a-profile"), "file").unwrap();
        let layout = Layout {
            manifest: root.join("installation.json"),
            profiles,
            root: root.clone(),
        };
        assert_eq!(list_names(&layout).unwrap(), ["default", "team-1"]);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn private_writer_uses_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let directory = std::env::temp_dir().join(format!(
            "artisan-cli-profile-mode-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir(&directory).unwrap();
        let destination = directory.join("secrets.json");
        write_private_json(&destination, &serde_json::json!({ "token": "secret" })).unwrap();
        assert_eq!(
            fs::metadata(&destination).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn private_writer_rejects_symlink_destinations() {
        use std::os::unix::fs::symlink;

        let directory = std::env::temp_dir().join(format!(
            "artisan-cli-profile-link-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir(&directory).unwrap();
        let target = directory.join("target");
        fs::write(&target, "unchanged").unwrap();
        let destination = directory.join("secrets.json");
        symlink(&target, &destination).unwrap();
        assert!(write_private_json(&destination, &serde_json::json!({})).is_err());
        assert_eq!(fs::read_to_string(target).unwrap(), "unchanged");
        fs::remove_dir_all(directory).unwrap();
    }
}
