use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineId {
    Claude,
    Codex,
    #[serde(rename = "opencode")]
    OpenCode,
    Grok,
    Cursor,
}

impl EngineId {
    pub const ALL: [Self; 5] = [
        Self::Claude,
        Self::Codex,
        Self::OpenCode,
        Self::Grok,
        Self::Cursor,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::OpenCode => "opencode",
            Self::Grok => "grok",
            Self::Cursor => "cursor",
        }
    }

    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
            Self::OpenCode => "OpenCode",
            Self::Grok => "Grok",
            Self::Cursor => "Cursor",
        }
    }
}

impl fmt::Display for EngineId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for EngineId {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|engine| {
                engine.as_str() == value || (value == "opencode2" && *engine == Self::OpenCode)
            })
            .ok_or_else(|| format!("unknown engine {value:?}"))
    }
}

/// How a thread runs. Engine-specific extras live in `options`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EngineConfig {
    pub engine: EngineId,
    pub model: Option<String>,
    /// One of the model's `efforts`.
    pub effort: Option<String>,
    #[serde(default)]
    pub permissions: Permissions,
    /// One of the model's `speeds` (its id). `None` is the model's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<String>,
    /// One of the model's `variants` (its id). `None` is the base model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    /// Engine-specific extras (Codex's `service_tier`, …).
    #[serde(default)]
    pub options: serde_json::Map<String, serde_json::Value>,
    /// Thread-owned adapter settings, never inherited by a new thread.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub session_options: serde_json::Map<String, serde_json::Value>,
}

impl EngineConfig {
    #[must_use]
    pub fn new(engine: EngineId) -> Self {
        Self {
            engine,
            model: None,
            effort: None,
            permissions: Permissions::Auto,
            speed: None,
            variant: None,
            options: serde_json::Map::new(),
            session_options: serde_json::Map::new(),
        }
    }

    /// The remembered model/effort preferences, without thread-scoped state.
    #[must_use]
    pub fn preference_config(&self) -> Self {
        Self {
            session_options: serde_json::Map::new(),
            ..self.clone()
        }
    }

    /// An explicit context override; absence keeps the engine's standard window.
    #[must_use]
    pub fn context_window_override(&self) -> Option<u64> {
        self.options
            .get("context_window")
            .and_then(serde_json::Value::as_u64)
            .filter(|tokens| (1..=100_000_000).contains(tokens))
    }

    #[must_use]
    pub fn option_str(&self, key: &str) -> Option<&str> {
        self.options.get(key).and_then(serde_json::Value::as_str)
    }
}

/// What an engine may do without asking.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permissions {
    /// The engine's own automatic mode: it works inside the project and
    /// decides itself what to bring to the user.
    #[default]
    Auto,
    /// Everything runs, nothing asks.
    Full,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    pub name: String,
    /// What the engine says the model is for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub efforts: Vec<String>,
    pub default_effort: Option<String>,
    /// What some of the `efforts` do, where the engine says.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effort_notes: Vec<EffortNote>,
    pub context_window: Option<u64>,
    /// Advertised context choices. An absent override is the standard window.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context_windows: Vec<ContextWindow>,
    #[serde(default)]
    pub supports_images: bool,
    /// Speed tiers, the default first. Empty when the model has one speed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub speeds: Vec<Speed>,
    /// Variants of this model the engine runs under the same id (OpenCode
    /// `model.variant`). Empty when there are none; the base model is not
    /// listed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variants: Vec<Variant>,
}

/// What one effort level does, as the engine describes it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffortNote {
    /// A separately presented special mode, advertised by the adapter.
    #[serde(default)]
    pub special: bool,
    /// One of the model's `efforts`.
    pub effort: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// A warning the engine attaches to it ("3.5× or more usage").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub advisory: Option<String>,
}

/// One speed tier of a model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Speed {
    /// What `EngineConfig::speed` carries; the engine's own tier id.
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The tier the model runs at when none is chosen.
    #[serde(default)]
    pub default: bool,
}

/// One variant of a model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Variant {
    /// What `EngineConfig::variant` carries.
    pub id: String,
    pub label: String,
}

/// What Forge knows about one engine right now. Not stored; sent on
/// connect and whenever it changes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EngineStatus {
    pub engine: EngineId,
    pub install: InstallState,
    pub sign_in: SignInState,
    pub models: std::sync::Arc<[Model]>,
    pub default_model: Option<String>,
    pub usage: Option<UsageLimits>,
    /// How this engine's downloads are verified. `None` when there is no
    /// build for this platform.
    #[serde(default)]
    pub integrity: Option<Integrity>,
    /// The vendor's published versions, newest first. Empty until asked for
    /// (the `versions` engine action).
    #[serde(default, skip_serializing_if = "<[PublishedVersion]>::is_empty")]
    pub versions: std::sync::Arc<[PublishedVersion]>,
}

/// What a download is checked against before it is installed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Integrity {
    /// The SHA-256 and size in the vendor's release manifest.
    VendorSha256,
    /// The SHA-512 `integrity` the npm registry publishes.
    NpmSha512,
    /// The vendor publishes no digest: HTTPS from the vendor's host only.
    HttpsOnly,
}

/// One published version and where it stands on this Forge.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishedVersion {
    pub version: String,
    /// On disk (current or previous).
    #[serde(default)]
    pub installed: bool,
    #[serde(default)]
    pub current: bool,
    /// Older than the oldest version Forge runs; can't be chosen.
    #[serde(default)]
    pub unsupported: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum InstallState {
    Absent,
    Downloading {
        version: String,
        received: u64,
        total: Option<u64>,
    },
    Verifying {
        version: String,
    },
    Installed {
        version: String,
        update: Option<String>,
    },
    Failed {
        message: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SignInState {
    Unknown,
    SignedOut,
    LoggingIn {
        url: Option<String>,
        code: Option<String>,
        #[serde(default)]
        accepts_code: bool,
    },
    SignedIn {
        account: Option<String>,
    },
    Failed {
        message: String,
    },
}

/// Rate-limit windows read from the engine. Best effort.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UsageLimits {
    pub windows: Vec<UsageWindow>,
    pub read_at: crate::Timestamp,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageCadence {
    Session,
    Week,
    #[default]
    Other,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UsageWindow {
    pub label: String,
    #[serde(default)]
    pub cadence: UsageCadence,
    #[serde(default)]
    pub scope: Option<String>,
    pub used_percent: f64,
    pub resets_at: Option<crate::Timestamp>,
}

#[cfg(test)]
mod shared_status_tests {
    use super::*;
    #[test]
    fn shared_catalog_wire_stays_an_array_and_empty_versions_stay_omitted() {
        let status: EngineStatus = serde_json::from_value(serde_json::json!({
            "engine":"claude","install":{"state":"absent"},"sign_in":{"state":"unknown"},
            "models":[],"default_model":null,"usage":null
        }))
        .unwrap();
        let copy = status.clone();
        assert!(std::sync::Arc::ptr_eq(&copy.models, &status.models));
        assert!(std::sync::Arc::ptr_eq(&copy.versions, &status.versions));
        let wire = serde_json::to_value(status).unwrap();
        assert_eq!(wire["models"], serde_json::json!([]));
        assert!(wire.get("versions").is_none());
    }
}

/// One context choice advertised by the engine's catalog.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextWindow {
    pub tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub override_tokens: Option<u64>,
}

impl Model {
    /// Drops a stale, malformed, or unsupported override after a model change.
    pub fn normalize_context_window(&self, config: &mut EngineConfig) {
        if config.options.contains_key("context_window")
            && !config.context_window_override().is_some_and(|tokens| {
                self.context_windows
                    .iter()
                    .any(|choice| choice.override_tokens == Some(tokens))
            })
        {
            config.options.remove("context_window");
        }
    }
}
