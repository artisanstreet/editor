//! Reading, atomic persistence, corrupt-file custody, and the one-time import
//! of the legacy loose `ui/` preference files.

use super::{EditorSettings, FPS_OVERLAY_KEY, FRAME_RATE_LIMIT_KEY, REOPEN_HOST_KEY};
use crate::native_frame_rate::FrameRateLimit;
use serde_json::{Map, Value};
use std::{
    io::Read as _,
    path::{Path, PathBuf},
};

const DIRECTORY_NAME: &str = "editor";
const FILE_NAME: &str = "settings.json";
/// Upper bound for the settings file; anything larger is treated as corrupt.
const MAX_FILE_BYTES: u64 = 256 * 1024;
/// Upper bound for one legacy preference file.
const MAX_LEGACY_BYTES: u64 = super::legacy_forge::MAX_FILE_BYTES;
const LEGACY_DIRECTORY_NAME: &str = "ui";
const LEGACY_FRAME_RATE_LIMIT: &str = "frame-rate-limit";
const LEGACY_FPS_OVERLAY: &str = "fps-overlay";
const LEGACY_REOPEN_HOST: &str = "last-used-host";

/// Why a settings change applied for this session only.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SettingsPersistError {
    #[error("the settings location is unavailable")]
    Unavailable,
    #[error("the settings file could not be written: {0}")]
    Io(#[source] std::io::Error),
}

/// The settings loaded for one install root.
#[derive(Debug)]
pub(crate) struct Loaded {
    pub(crate) settings: EditorSettings,
    /// Where changes persist; `None` keeps them for this session only.
    pub(crate) path: Option<PathBuf>,
    /// Non-fatal problems met while loading.
    pub(crate) diagnostics: Vec<String>,
}

impl Loaded {
    pub(super) fn detached(diagnostics: Vec<String>) -> Self {
        Self {
            settings: EditorSettings::default(),
            path: None,
            diagnostics,
        }
    }
}

pub(crate) fn settings_path(root: &Path) -> PathBuf {
    root.join(DIRECTORY_NAME).join(FILE_NAME)
}

enum FileRead {
    Missing,
    Object(Map<String, Value>),
    Corrupt(String),
    Unreadable(std::io::Error),
}

/// Loads `<root>/editor/settings.json`, importing and then removing the legacy
/// loose preference files once. Never fails: problems become diagnostics and
/// the affected values fall back to their defaults.
pub(crate) fn load(root: &Path) -> Loaded {
    let path = settings_path(root);
    let mut diagnostics = Vec::new();
    let (object, writable) = match read_settings(&path) {
        FileRead::Missing => (Map::new(), true),
        FileRead::Object(object) => (object, true),
        FileRead::Corrupt(problem) => {
            let kept = path.with_extension("json.corrupt");
            match std::fs::rename(&path, &kept) {
                Ok(()) => {
                    diagnostics.push(format!(
                        "{} is corrupt ({problem}); kept it as {} and using defaults",
                        path.display(),
                        kept.display()
                    ));
                    (Map::new(), true)
                }
                Err(error) => {
                    diagnostics.push(format!(
                        "{} is corrupt ({problem}) and could not be set aside ({error}); \
                         using defaults without saving changes",
                        path.display()
                    ));
                    (Map::new(), false)
                }
            }
        }
        FileRead::Unreadable(error) => {
            diagnostics.push(format!(
                "{} is unreadable ({error}); using defaults without saving changes",
                path.display()
            ));
            (Map::new(), false)
        }
    };
    let mut settings = EditorSettings::from_object(object.clone());
    let legacy = root.join(LEGACY_DIRECTORY_NAME);
    let imported = import_legacy(&mut settings, &object, &legacy);
    if writable {
        let migrated = !imported
            || {
                let result = persist(&path, &settings);
                if let Err(error) = &result {
                    diagnostics.push(format!(
                    "imported legacy preferences could not be saved ({error}); keeping the legacy files"
                ));
                }
                result.is_ok()
            };
        if migrated {
            remove_legacy(&legacy, &mut diagnostics);
        }
    }
    Loaded {
        settings,
        path: writable.then_some(path),
        diagnostics,
    }
}

fn read_settings(path: &Path) -> FileRead {
    let bytes = match read_bounded(path, MAX_FILE_BYTES) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return FileRead::Corrupt(format!("larger than {MAX_FILE_BYTES} bytes")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return FileRead::Missing,
        Err(error) => return FileRead::Unreadable(error),
    };
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(Value::Object(object)) => FileRead::Object(object),
        Ok(_) => FileRead::Corrupt("not a JSON object".into()),
        Err(error) => FileRead::Corrupt(error.to_string()),
    }
}

/// Reads at most `limit` bytes; `Ok(None)` when the file is larger.
fn read_bounded(path: &Path, limit: u64) -> std::io::Result<Option<Vec<u8>>> {
    let file = std::fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    Ok((bytes.len() as u64 <= limit).then_some(bytes))
}

/// Writes the settings through a temporary sibling, then renames it over the
/// file, so readers only ever see a complete document.
pub(super) fn persist(path: &Path, settings: &EditorSettings) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut bytes =
        serde_json::to_vec_pretty(&settings.to_json()).map_err(std::io::Error::other)?;
    bytes.push(b'\n');
    let pending = path.with_extension("json.pending");
    std::fs::write(&pending, bytes)?;
    std::fs::rename(&pending, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&pending);
    })
}

/// Imports each legacy value the settings file does not already hold.
/// Returns whether anything was imported.
fn import_legacy(
    settings: &mut EditorSettings,
    stored: &Map<String, Value>,
    legacy: &Path,
) -> bool {
    let absent = |key: &str| !stored.contains_key(key);
    let read = |name: &str| {
        read_bounded(&legacy.join(name), MAX_LEGACY_BYTES)
            .ok()
            .flatten()
    };
    let mut imported = false;
    if absent(FRAME_RATE_LIMIT_KEY)
        && let Some(limit) = read(LEGACY_FRAME_RATE_LIMIT)
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .and_then(|text| FrameRateLimit::parse(&text))
    {
        *settings = settings.clone().with_frame_rate_limit(limit);
        imported = true;
    }
    if absent(FPS_OVERLAY_KEY)
        && let Some(bytes) = read(LEGACY_FPS_OVERLAY)
    {
        // The legacy reader showed the overlay unless the file said `false`.
        let visible = String::from_utf8_lossy(&bytes).trim() != "false";
        *settings = settings.clone().with_fps_overlay(visible);
        imported = true;
    }
    if absent(REOPEN_HOST_KEY)
        && let Some(bytes) = read(LEGACY_REOPEN_HOST)
    {
        // An empty legacy file recorded the local machine.
        let home = std::str::from_utf8(&bytes)
            .ok()
            .and_then(super::host_from_text);
        *settings = settings.clone().with_reopen_host(home);
        imported = true;
    }
    imported
}

/// Reads one legacy Forge-pool preference file; `None` when it is absent,
/// unreadable, or oversized.
pub(super) fn read_legacy_file(path: &Path) -> Option<Vec<u8>> {
    read_bounded(path, super::legacy_forge::MAX_FILE_BYTES)
        .ok()
        .flatten()
}

/// Removes legacy Forge-pool preference files the Forge has answered for,
/// returning a problem for each file that could not be removed.
pub(super) fn remove_legacy_files(paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .filter_map(|path| match std::fs::remove_file(path) {
            Ok(()) => None,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => Some(format!(
                "legacy preference {} could not be removed ({error})",
                path.display()
            )),
        })
        .collect()
}

fn remove_legacy(legacy: &Path, diagnostics: &mut Vec<String>) {
    for name in [
        LEGACY_FRAME_RATE_LIMIT,
        LEGACY_FPS_OVERLAY,
        LEGACY_REOPEN_HOST,
    ] {
        let path = legacy.join(name);
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => diagnostics.push(format!(
                "legacy preference {} could not be removed ({error})",
                path.display()
            )),
        }
    }
}
