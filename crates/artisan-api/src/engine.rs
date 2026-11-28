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
    Antigravity,
}

impl EngineId {
    pub const ALL: [Self; 4] = [Self::Claude, Self::Codex, Self::OpenCode, Self::Antigravity];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::OpenCode => "opencode",
            Self::Antigravity => "antigravity",
        }
    }

    #[must_use]
    pub const fn enabled_setting(self) -> &'static str {
        match self {
            Self::Claude => "claude_enabled",
            Self::Codex => "codex_enabled",
            Self::OpenCode => "opencode_enabled",
            Self::Antigravity => "antigravity_enabled",
        }
    }

    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
            Self::OpenCode => "OpenCode",
            Self::Antigravity => "Antigravity",
        }
    }
}

impl fmt::Display for EngineId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineConfig {
    pub engine: EngineId,
    pub model: Option<String>,
    pub effort: Option<String>,
    #[serde(default)]
    pub permissions: Permissions,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    #[serde(default)]
    pub options: serde_json::Map<String, serde_json::Value>,
}

impl EngineConfig {
    pub fn compaction_window_override(&self) -> Option<u64> {
        self.options
            .get("compaction_window")
            .and_then(serde_json::Value::as_u64)
            .filter(|tokens| (1..=100_000_000).contains(tokens))
    }

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

    #[must_use]
    pub fn context_window_override(&self) -> Option<u64> {
        self.options
            .get("context_window")
            .and_then(serde_json::Value::as_u64)
            .filter(|tokens| (1..=100_000_000).contains(tokens))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permissions {
    #[default]
    Auto,
    Full,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Model {
    #[serde(default)]
    pub evidence: crate::ModelEvidence,
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub efforts: Vec<String>,
    pub default_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effort_notes: Vec<EffortNote>,
    pub context_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context_windows: Vec<ContextWindow>,
    #[serde(default)]
    pub supports_images: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub speeds: Vec<Speed>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variants: Vec<Variant>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_variant: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lab: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffortNote {
    #[serde(default)]
    pub special: bool,
    pub effort: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub advisory: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Speed {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub default: bool,
}

impl Model {
    pub fn variant_for(&self, chosen: Option<&str>) -> Option<String> {
        chosen
            .filter(|chosen| self.variants.iter().any(|variant| variant.id == *chosen))
            .map(str::to_owned)
            .or_else(|| self.default_variant.clone())
    }

    pub fn fit(&self, config: &mut EngineConfig) {
        *config = self.resolve_config(config, &[]).effective;
    }
}

pub const MODEL_CONFIGS_KEY: &str = "model_engine_configs";

pub const HARNESS_CONFIGS_KEY: &str = "harness_engine_configs";

pub const LAST_PROJECT_KEY: &str = "last_project";

pub const MODEL_GROUPS_KEY: &str = "model_groups";

#[must_use]
pub fn model_config_key(engine: EngineId, model: &str) -> String {
    format!("{engine}/{model}")
}

#[must_use]
pub fn saved_model_config<'a>(
    configs: &'a std::collections::BTreeMap<String, EngineConfig>,
    engine: EngineId,
    model: &str,
) -> Option<&'a EngineConfig> {
    configs
        .get(&model_config_key(engine, model))
        .into_iter()
        .chain(configs.values())
        .find(|config| config.engine == engine && config.model.as_deref() == Some(model))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Variant {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EngineStatus {
    #[serde(default)]
    pub discovery: crate::DiscoveryState,
    pub engine: EngineId,
    pub install: InstallState,
    pub sign_in: SignInState,
    pub models: std::sync::Arc<[Model]>,
    pub default_model: Option<String>,
    #[serde(default, skip_serializing_if = "<[SlashCommand]>::is_empty")]
    pub commands: std::sync::Arc<[SlashCommand]>,
    pub usage: Option<UsageLimits>,
    #[serde(default)]
    pub integrity: Option<Integrity>,
    #[serde(default, skip_serializing_if = "<[PublishedVersion]>::is_empty")]
    pub versions: std::sync::Arc<[PublishedVersion]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_ai: Option<crate::ClaudeAiState>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlashCommand {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Integrity {
    VendorSha256,
    VendorSha512,
    NpmSha512,
    ArtisanSha256,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishedVersion {
    pub version: String,
    #[serde(default)]
    pub installed: bool,
    #[serde(default)]
    pub current: bool,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        plan: Option<String>,
    },
    Failed {
        message: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UsageLimits {
    pub windows: Vec<UsageWindow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spend: Option<Box<UsageSpend>>,
    pub read_at: crate::Timestamp,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageSpend {
    pub periods: Vec<SpendLine>,
    #[serde(default)]
    pub models: Vec<SpendLine>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpendLine {
    pub label: String,
    pub cost_usd: f64,
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
#[path = "../tests/engine/shared_status.rs"]
mod shared_status_tests;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextWindow {
    pub tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub override_tokens: Option<u64>,
}

impl Model {
    pub fn normalize_context_window(&self, config: &mut EngineConfig) {
        if let Some(migrated) = self.migrate_config(config) {
            *config = migrated;
        }
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
