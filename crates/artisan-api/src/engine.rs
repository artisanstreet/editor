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
}

impl EngineId {
    pub const ALL: [Self; 3] = [Self::Claude, Self::Codex, Self::OpenCode];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::OpenCode => "opencode",
        }
    }

    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
            Self::OpenCode => "OpenCode",
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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
    /// The variant a config without one runs. Set whenever `variants` is
    /// not empty: the picker offers no separate "Default".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_variant: Option<String>,
    /// Who made the model, where one engine serves several labs' models.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lab: Option<String>,
    /// The gateway the engine serves it through ("OpenCode Go"); the
    /// picker groups models by it. Absent when the engine has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
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

impl Model {
    /// The variant a config choosing `chosen` runs: that one while this
    /// model offers it, else the model's default.
    pub fn variant_for(&self, chosen: Option<&str>) -> Option<String> {
        chosen
            .filter(|chosen| self.variants.iter().any(|variant| variant.id == *chosen))
            .map(str::to_owned)
            .or_else(|| self.default_variant.clone())
    }

    /// Fits a config chosen for this model earlier to what it offers now:
    /// an effort, speed, variant or context window it no longer offers
    /// falls back to the model's own.
    pub fn fit(&self, config: &mut EngineConfig) {
        self.normalize_context_window(config);
        if config
            .effort
            .as_ref()
            .is_some_and(|effort| !self.efforts.contains(effort))
        {
            config.effort.clone_from(&self.default_effort);
        }
        if config
            .speed
            .as_ref()
            .is_some_and(|speed| !self.speeds.iter().any(|offered| &offered.id == speed))
        {
            config.speed = None;
        }
        config.variant = self.variant_for(config.variant.as_deref());
    }
}

/// The Forge setting holding the config last chosen for each model.
///
/// A model comes back with its own effort, speed, context and permissions.
/// Entries are found by the engine and model they hold; Forge names new
/// ones `engine/model`.
pub const MODEL_CONFIGS_KEY: &str = "model_engine_configs";

/// The Forge setting naming the project the user last started a thread in.
pub const LAST_PROJECT_KEY: &str = "last_project";

/// The Forge setting holding which of an engine's model groups the user
/// opened (`true`) or closed: `{engine: {group: open}}`.
pub const MODEL_GROUPS_KEY: &str = "model_groups";

/// The name Forge files `config` under in `MODEL_CONFIGS_KEY`.
#[must_use]
pub fn model_config_key(engine: EngineId, model: &str) -> String {
    format!("{engine}/{model}")
}

/// The config last chosen for `engine`'s `model` in a `MODEL_CONFIGS_KEY`
/// value, as it was saved.
#[must_use]
pub fn saved_model_config(
    configs: &serde_json::Value,
    engine: EngineId,
    model: &str,
) -> Option<EngineConfig> {
    let configs = configs.as_object()?;
    configs
        .get(&model_config_key(engine, model))
        .into_iter()
        .chain(configs.values())
        .filter_map(|value| serde_json::from_value::<EngineConfig>(value.clone()).ok())
        .find(|config| config.engine == engine && config.model.as_deref() == Some(model))
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
    /// The `/` commands the engine reads, as it reports them, plus the ones
    /// Forge reads for it. Empty until the engine has said.
    #[serde(default, skip_serializing_if = "<[SlashCommand]>::is_empty")]
    pub commands: std::sync::Arc<[SlashCommand]>,
    pub usage: Option<UsageLimits>,
    /// How this engine's downloads are verified. `None` when there is no
    /// build for this platform.
    #[serde(default)]
    pub integrity: Option<Integrity>,
    /// The vendor's published versions, newest first. Empty until asked for
    /// (the `versions` engine action).
    #[serde(default, skip_serializing_if = "<[PublishedVersion]>::is_empty")]
    pub versions: std::sync::Arc<[PublishedVersion]>,
    /// The claude.ai session Native Claude Mode runs on. Claude only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_ai: Option<crate::ClaudeAiState>,
}

/// One `/` command an engine reads at the start of a message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlashCommand {
    /// Without the `/`: `compact`, `cloudflare:wrangler`.
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// What to type after it (`<objective>`), when the engine says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// What a download is checked against before it is installed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Integrity {
    /// The SHA-256 and size in the vendor's release manifest.
    VendorSha256,
    /// The SHA-512 `integrity` the npm registry publishes.
    NpmSha512,
    /// The SHA-256 and size in the manifest of Artisan's own build of the
    /// vendor's release.
    ArtisanSha256,
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
    /// What the engine says it spent, when it prices its own calls
    /// (`OpenCode`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spend: Option<Box<UsageSpend>>,
    pub read_at: crate::Timestamp,
}

/// Spend the engine recorded on this host, newest period first.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageSpend {
    /// `Today`, `Last 7 days`, `Last 30 days`.
    pub periods: Vec<SpendLine>,
    /// The longest period by model, costliest first.
    #[serde(default)]
    pub models: Vec<SpendLine>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpendLine {
    pub label: String,
    pub cost_usd: f64,
    /// Input, output and reasoning tokens; cache reads are left out.
    pub tokens: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageCadence {
    Session,
    Week,
    Month,
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
