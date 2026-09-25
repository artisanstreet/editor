//! Editor pool: device-local presentation settings beside the installation.
//!
//! See `docs/plans/stateless-editor.md` section 1. One typed, schema-versioned
//! [`EditorSettings`] value lives at `<install root>/editor/settings.json`
//! (beside `versions/`, so it survives updates and rollback). It is loaded once
//! per process, installed into a GPUI global, and replaced — never mutated in
//! place — through [`update`], which applies the new value and then persists it
//! atomically. Keys this build does not understand are carried through
//! unchanged so older and newer Editor builds can share the file.
//!
//! Only this module may write the file; `file_write_guard_tests` enforces that
//! Editor code does not write files elsewhere.

use crate::native_frame_rate::FrameRateLimit;
use gpui::{App, Global};
use serde_json::{Map, Value};
use std::{
    path::{Path, PathBuf},
    sync::{LazyLock, OnceLock},
};

mod legacy_forge;
mod storage;
pub(crate) use legacy_forge::LegacyForgePreferences;
pub(crate) use storage::{Loaded, SettingsPersistError};
#[cfg(test)]
pub(crate) use storage::{load, settings_path};

/// The Forge-pool preferences an older Editor kept in loose files for the
/// host whose credential home is `home` (`None` for this computer). Empty
/// without an install root, and in unit tests, which never touch it.
pub(crate) fn legacy_forge_preferences(home: Option<&Path>) -> LegacyForgePreferences {
    if cfg!(test) {
        return LegacyForgePreferences::default();
    }
    artisan_editor_cli::paths::Layout::discover().map_or_else(
        |_| LegacyForgePreferences::default(),
        |layout| legacy_forge::find(&layout.root, home, storage::read_legacy_file),
    )
}

/// The schema this build writes. Newer files keep their higher version.
pub(crate) const SCHEMA_VERSION: u64 = 1;
const VERSION_KEY: &str = "version";
const FRAME_RATE_LIMIT_KEY: &str = "frame_rate_limit";
const FPS_OVERLAY_KEY: &str = "fps_overlay";
const REOPEN_HOST_KEY: &str = "reopen_host";

/// Editor-only presentation state. Every field has a default, so a missing,
/// partial, or foreign file always yields a usable value.
///
/// A future schema that changes the meaning or type of a known key must use a
/// new key: a known key whose value this build cannot read falls back to its
/// default and is rewritten in this build's format.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EditorSettings {
    version: u64,
    frame_rate_limit: FrameRateLimit,
    fps_overlay: bool,
    reopen_host: Option<PathBuf>,
    /// Keys this build does not understand, written back verbatim.
    unknown: Map<String, Value>,
}

impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            version: SCHEMA_VERSION,
            frame_rate_limit: FrameRateLimit::default(),
            fps_overlay: true,
            reopen_host: None,
            unknown: Map::new(),
        }
    }
}

impl EditorSettings {
    pub(crate) fn frame_rate_limit(&self) -> FrameRateLimit {
        self.frame_rate_limit
    }

    pub(crate) fn fps_overlay(&self) -> bool {
        self.fps_overlay
    }

    /// The host to reopen at launch: `None` is the local machine (or no hint),
    /// `Some` is a registered remote host home.
    pub(crate) fn reopen_host(&self) -> Option<&Path> {
        self.reopen_host.as_deref()
    }

    #[must_use]
    pub(crate) fn with_frame_rate_limit(self, frame_rate_limit: FrameRateLimit) -> Self {
        Self {
            frame_rate_limit,
            ..self
        }
    }

    #[must_use]
    pub(crate) fn with_fps_overlay(self, fps_overlay: bool) -> Self {
        Self {
            fps_overlay,
            ..self
        }
    }

    /// Records the host to reopen. Homes that cannot be stored as UTF-8 text
    /// fall back to the local machine rather than persisting a lossy path.
    #[must_use]
    pub(crate) fn with_reopen_host(self, home: Option<PathBuf>) -> Self {
        Self {
            reopen_host: home.filter(|home| home.to_str().is_some_and(|text| !text.is_empty())),
            ..self
        }
    }

    /// Reads one settings object leniently: each known key falls back to its
    /// default on its own, and every other key is retained verbatim.
    fn from_object(mut unknown: Map<String, Value>) -> Self {
        let defaults = Self::default();
        let version = unknown
            .remove(VERSION_KEY)
            .and_then(|value| value.as_u64())
            .unwrap_or(SCHEMA_VERSION);
        let frame_rate_limit = unknown
            .remove(FRAME_RATE_LIMIT_KEY)
            .and_then(|value| value.as_str().and_then(FrameRateLimit::parse))
            .unwrap_or(defaults.frame_rate_limit);
        let fps_overlay = unknown
            .remove(FPS_OVERLAY_KEY)
            .and_then(|value| value.as_bool())
            .unwrap_or(defaults.fps_overlay);
        let reopen_host = unknown
            .remove(REOPEN_HOST_KEY)
            .and_then(|value| value.as_str().and_then(host_from_text));
        Self {
            version,
            frame_rate_limit,
            fps_overlay,
            reopen_host,
            unknown,
        }
    }

    fn to_json(&self) -> Value {
        let mut object = self.unknown.clone();
        object.insert(
            VERSION_KEY.into(),
            Value::from(self.version.max(SCHEMA_VERSION)),
        );
        object.insert(
            FRAME_RATE_LIMIT_KEY.into(),
            Value::String(self.frame_rate_limit.label()),
        );
        object.insert(FPS_OVERLAY_KEY.into(), Value::Bool(self.fps_overlay));
        object.insert(
            REOPEN_HOST_KEY.into(),
            self.reopen_host
                .as_deref()
                .and_then(Path::to_str)
                .map_or(Value::Null, |home| Value::String(home.to_owned())),
        );
        Value::Object(object)
    }
}

fn host_from_text(text: &str) -> Option<PathBuf> {
    (!text.is_empty() && !text.contains('\0')).then(|| PathBuf::from(text))
}

struct EditorSettingsStore {
    settings: EditorSettings,
    /// `None` keeps changes for this session only (no install root, or a file
    /// that must not be overwritten).
    path: Option<PathBuf>,
}
impl Global for EditorSettingsStore {}

static DEFAULTS: LazyLock<EditorSettings> = LazyLock::new(EditorSettings::default);
static STARTUP: OnceLock<Loaded> = OnceLock::new();

fn startup_load() -> &'static Loaded {
    STARTUP.get_or_init(|| {
        // Unit tests never touch the developer's install root.
        if cfg!(test) {
            return Loaded::detached(Vec::new());
        }
        let loaded = match artisan_editor_cli::paths::Layout::discover() {
            Ok(layout) => storage::load(&layout.root),
            Err(error) => Loaded::detached(vec![format!(
                "install root unavailable ({error}); changes apply for this session only"
            )]),
        };
        for diagnostic in &loaded.diagnostics {
            eprintln!("editor settings: {diagnostic}");
        }
        loaded
    })
}

/// The value loaded once for this process, for launch decisions made before a
/// GPUI context exists (such as which host to open). Later changes are only
/// visible through [`get`].
pub(crate) fn startup() -> &'static EditorSettings {
    &startup_load().settings
}

/// Installs the process's loaded settings as the GPUI global.
pub(crate) fn initialize(cx: &mut App) {
    let loaded = startup_load();
    install(loaded.settings.clone(), loaded.path.clone(), cx);
}

/// Installs one settings value and its persistence target.
pub(crate) fn install(settings: EditorSettings, path: Option<PathBuf>, cx: &mut App) {
    cx.set_global(EditorSettingsStore { settings, path });
}

/// The current settings; defaults before [`initialize`].
pub(crate) fn get(cx: &App) -> &EditorSettings {
    cx.try_global::<EditorSettingsStore>()
        .map_or(&*DEFAULTS, |store| &store.settings)
}

/// Replaces the settings with `change(current)`, applies the new value, then
/// persists it. On error the new value still applies for this session.
///
/// # Errors
///
/// Returns [`SettingsPersistError`] when the value could not be saved.
pub(crate) fn update(
    cx: &mut App,
    change: impl FnOnce(EditorSettings) -> EditorSettings,
) -> Result<(), SettingsPersistError> {
    let path = cx
        .try_global::<EditorSettingsStore>()
        .and_then(|store| store.path.clone());
    let settings = change(get(cx).clone());
    cx.set_global(EditorSettingsStore { settings, path });
    let store = cx.global::<EditorSettingsStore>();
    let path = store
        .path
        .as_deref()
        .ok_or(SettingsPersistError::Unavailable)?;
    storage::persist(path, &store.settings).map_err(SettingsPersistError::Io)
}

#[cfg(test)]
#[path = "editor_settings/tests.rs"]
mod tests;
