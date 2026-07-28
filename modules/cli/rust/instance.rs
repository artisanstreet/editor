use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};

use crate::{CliError, Result, error::io, paths::Layout};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct InstanceConfig {
    pub data_root: PathBuf,
    pub listen_host: String,
    pub listen_port: u16,
    pub mode: ForgeMode,
    /// Static web hosting is a development capability. Installed homes
    /// default to a control-surface-only Forge; the Electron editor renders
    /// the bundled frontend instead of a Forge-served page.
    #[serde(default)]
    pub serve_frontend: bool,
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
pub struct InstancePaths {
    pub config: PathBuf,
    pub secrets: PathBuf,
    pub state: PathBuf,
    pub log: PathBuf,
}

/// Resolves the home's single Forge instance files. This is the path
/// resolution choke point, so the legacy `profiles/<name>/` layout migrates
/// here before any caller reads or writes an instance file.
pub fn paths(layout: &Layout) -> Result<InstancePaths> {
    migrate_legacy_profiles(layout)?;
    Ok(InstancePaths {
        config: layout.root.join("config.json"),
        secrets: layout.root.join("secrets.json"),
        state: layout.root.join("state.json"),
        log: layout.root.join("forge.log"),
    })
}

/// Moves a single legacy `profiles/<name>/` directory's contents to the home
/// root. A home that already has a root `config.json` is current and skipped.
/// More than one legacy profile cannot be merged automatically, so the user
/// must delete all but one before any command proceeds.
fn migrate_legacy_profiles(layout: &Layout) -> Result<()> {
    if layout.root.join("config.json").is_file() {
        return Ok(());
    }
    let profiles = layout.root.join("profiles");
    if !profiles.is_dir() {
        return Ok(());
    }
    let mut directories = Vec::new();
    for entry in fs::read_dir(&profiles).map_err(io("enumerate legacy Forge profiles"))? {
        let entry = entry.map_err(io("enumerate legacy Forge profile"))?;
        let file_type = entry
            .file_type()
            .map_err(io("inspect legacy Forge profile"))?;
        if file_type.is_dir() && !file_type.is_symlink() {
            directories.push(entry.path());
        }
    }
    match directories.as_slice() {
        [] => {
            let _ = fs::remove_dir_all(&profiles);
            Ok(())
        }
        [single] => {
            let name = single
                .file_name()
                .map(|value| value.to_string_lossy().into_owned())
                .unwrap_or_default();
            for entry in fs::read_dir(single).map_err(io("enumerate legacy Forge profile"))? {
                let entry = entry.map_err(io("enumerate legacy Forge profile"))?;
                let destination = layout.root.join(entry.file_name());
                if destination.exists() {
                    return Err(CliError::Installation(format!(
                        "cannot migrate legacy Forge profile `{name}`: {} already exists",
                        destination.display()
                    )));
                }
                fs::rename(entry.path(), &destination)
                    .map_err(io("migrate legacy Forge profile file"))?;
            }
            let _ = fs::remove_dir_all(&profiles);
            eprintln!(
                "migrated legacy Forge profile `{name}` into the Artisan home root at {}",
                layout.root.display()
            );
            Ok(())
        }
        many => {
            let names = many
                .iter()
                .filter_map(|path| path.file_name())
                .map(|name| name.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(", ");
            Err(CliError::Installation(format!(
                "this Artisan home has multiple legacy Forge profiles ({names}) and Artisan now \
                 runs one Forge per home; delete all but one directory under {} and retry",
                profiles.display()
            )))
        }
    }
}

pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = fs::read(path).map_err(io("read Forge instance file"))?;
    serde_json::from_slice(&bytes).map_err(|source| CliError::Json {
        path: path.to_path_buf(),
        source,
    })
}

pub fn load(layout: &Layout) -> Result<(InstancePaths, InstanceConfig, Secrets)> {
    let paths = paths(layout)?;
    if !paths.config.is_file() || !paths.secrets.is_file() {
        return Err(CliError::MissingInstance);
    }
    let config = read_json(&paths.config)?;
    let secrets = read_json(&paths.secrets)?;
    Ok((paths, config, secrets))
}

pub fn setup(
    layout: &Layout,
    mode: ForgeMode,
    port: u16,
    data_root: Option<&Path>,
    serve_frontend: bool,
) -> Result<()> {
    let paths = paths(layout)?;
    fs::create_dir_all(&layout.root).map_err(io("create Artisan home directory"))?;
    restrict_directory(&layout.root)?;
    let config = InstanceConfig {
        data_root: data_root.map_or_else(|| layout.root.join("data"), Path::to_path_buf),
        listen_host: "127.0.0.1".into(),
        listen_port: port,
        mode,
        serve_frontend,
        version: 1,
    };
    write_private_json(&paths.config, &config)?;
    if !paths.secrets.exists() {
        let mut token = [0_u8; 32];
        getrandom::fill(&mut token).map_err(|error| {
            CliError::Installation(format!("secure random source failed: {error}"))
        })?;
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
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("instance"),
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
            .map_err(io("create temporary instance file"))?;
        file.write_all(&bytes)
            .map_err(io("write temporary instance file"))?;
        file.sync_all()
            .map_err(io("sync temporary instance file"))?;
        drop(file);
        reject_unsafe_destination(path)?;
        fs::rename(&temporary, path).map_err(io("activate instance file"))?;
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
            context: "inspect instance file destination",
            source,
        }),
    }
}

#[cfg(unix)]
fn restrict_directory(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(io("restrict Artisan home directory"))
}

#[cfg(not(unix))]
fn restrict_directory(path: &Path) -> Result<()> {
    // The installer places ARTISAN_HOME in a current-user directory. Rust's
    // safe standard library has no Windows ACL API; do not broaden its ACL.
    fs::metadata(path)
        .map(|_| ())
        .map_err(io("inspect Artisan home directory"))
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(io("sync Artisan home directory"))
}

#[cfg(not(unix))]
fn sync_directory(path: &Path) -> Result<()> {
    fs::metadata(path)
        .map(|_| ())
        .map_err(io("inspect Artisan home directory"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_layout(label: &str) -> Layout {
        let root = std::env::temp_dir().join(format!(
            "artisan-cli-{label}-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        Layout {
            manifest: root.join("installation.json"),
            root,
        }
    }

    #[test]
    fn configs_without_the_flag_never_serve_the_frontend() {
        let config: InstanceConfig = serde_json::from_str(
            r#"{"data_root":"C:/data","listen_host":"127.0.0.1","listen_port":0,"mode":"local","version":1}"#,
        )
        .expect("legacy instance config");
        assert!(!config.serve_frontend);
    }

    #[test]
    fn private_writer_replaces_regular_files_without_leaving_temporary_files() {
        let directory =
            std::env::temp_dir().join(format!("artisan-cli-instance-test-{}", std::process::id()));
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
    fn a_single_legacy_profile_migrates_to_the_home_root() {
        let layout = temporary_layout("migrate-single");
        let profile = layout.root.join("profiles").join("browser-dev");
        fs::create_dir_all(profile.join("data")).unwrap();
        fs::write(profile.join("config.json"), b"{\"version\":1}").unwrap();
        fs::write(profile.join("secrets.json"), b"{\"version\":1}").unwrap();
        fs::write(profile.join("data").join("artisan.sqlite"), b"db").unwrap();

        let paths = paths(&layout).unwrap();

        assert!(paths.config.is_file());
        assert!(layout.root.join("secrets.json").is_file());
        assert!(layout.root.join("data").join("artisan.sqlite").is_file());
        assert!(!layout.root.join("profiles").exists());
        fs::remove_dir_all(layout.root).unwrap();
    }

    #[test]
    fn multiple_legacy_profiles_fail_with_an_actionable_error() {
        let layout = temporary_layout("migrate-multiple");
        fs::create_dir_all(layout.root.join("profiles").join("default")).unwrap();
        fs::create_dir_all(layout.root.join("profiles").join("team-1")).unwrap();

        let error = paths(&layout).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("default"), "{message}");
        assert!(message.contains("team-1"), "{message}");
        assert!(message.contains("delete all but one"), "{message}");
        fs::remove_dir_all(layout.root).unwrap();
    }

    #[test]
    fn a_migrated_home_ignores_leftover_legacy_directories() {
        let layout = temporary_layout("migrate-noop");
        fs::write(layout.root.join("config.json"), b"{\"version\":1}").unwrap();
        fs::create_dir_all(layout.root.join("profiles").join("default")).unwrap();
        fs::create_dir_all(layout.root.join("profiles").join("team-1")).unwrap();

        let paths = paths(&layout).unwrap();

        assert!(paths.config.is_file());
        fs::remove_dir_all(layout.root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn private_writer_uses_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let directory = std::env::temp_dir().join(format!(
            "artisan-cli-instance-mode-test-{}",
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
            "artisan-cli-instance-link-test-{}",
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
