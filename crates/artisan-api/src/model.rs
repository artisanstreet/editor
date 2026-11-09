mod configuration;
mod customization;
mod specification;

pub use customization::validate_model_customizations;

use crate::{EngineConfig, EngineId, Model, Permissions, Timestamp};
use serde::{Deserialize, Serialize};

pub const HIDE_BAD_MODEL_OPTIONS_KEY: &str = "hide_bad_model_options";
pub const MODEL_CUSTOMIZATIONS_KEY: &str = "model_customizations";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingLevel {
    Light,
    Medium,
    High,
    ExtraHigh,
    Max,
}

impl ThinkingLevel {
    pub fn from_native(id: &str) -> Option<Self> {
        match id {
            "minimal" | "low" | "light" => Some(Self::Light),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            "xhigh" => Some(Self::ExtraHigh),
            "max" => Some(Self::Max),
            _ => None,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Light => "Light",
            Self::Medium => "Medium",
            Self::High => "High",
            Self::ExtraHigh => "Extra High",
            Self::Max => "Max",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeedLevel {
    Normal,
    Fast,
    Superfast,
}

impl SpeedLevel {
    pub fn from_native(id: &str) -> Option<Self> {
        match id {
            "standard" | "default" | "normal" => Some(Self::Normal),
            "fast" | "priority" => Some(Self::Fast),
            "ultrafast" => Some(Self::Superfast),
            _ => None,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Normal => "Normal",
            Self::Fast => "Fast",
            Self::Superfast => "Superfast",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelAxis {
    Activation,
    Thinking,
    Special,
    Variant,
    Speed,
    Context,
    Compaction,
    Permission,
}

impl ModelAxis {
    pub const ALL: [Self; 8] = [
        Self::Activation,
        Self::Thinking,
        Self::Special,
        Self::Variant,
        Self::Speed,
        Self::Context,
        Self::Compaction,
        Self::Permission,
    ];

    pub const fn title(self) -> &'static str {
        match self {
            Self::Activation => "Thinking",
            Self::Thinking => "Effort",
            Self::Special => "Special mode",
            Self::Variant => "Variant",
            Self::Speed => "Speed",
            Self::Context => "Context limit",
            Self::Compaction => "Compaction window",
            Self::Permission => "Access",
        }
    }

    pub const fn shown(self, options: usize) -> bool {
        options > 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Capability<T> {
    Unknown,
    Unsupported,
    Supported(T),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelEvidence {
    pub sources: Vec<FieldSource>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldSource {
    pub field: String,
    pub source: String,
    pub observed_at: Timestamp,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DiscoveryState {
    #[default]
    Unknown,
    Refreshing {
        last_success_at: Option<Timestamp>,
    },
    Fresh {
        observed_at: Timestamp,
    },
    Stale {
        last_success_at: Timestamp,
        error: String,
    },
    Unavailable {
        error: String,
    },
}

impl DiscoveryState {
    pub const fn last_success_at(&self) -> Option<Timestamp> {
        match self {
            Self::Fresh { observed_at } => Some(*observed_at),
            Self::Stale {
                last_success_at, ..
            } => Some(*last_success_at),
            Self::Refreshing { last_success_at } => *last_success_at,
            Self::Unknown | Self::Unavailable { .. } => None,
        }
    }

    #[must_use]
    pub fn failed(&self, error: &str) -> Self {
        self.last_success_at().map_or_else(
            || Self::Unavailable {
                error: error.to_owned(),
            },
            |last_success_at| Self::Stale {
                last_success_at,
                error: error.to_owned(),
            },
        )
    }

    pub fn description(&self, now: Timestamp) -> String {
        let age = |at: Timestamp| (now.saturating_sub(at) / 1000).max(0);
        match self {
            Self::Unknown => "Model discovery has not completed.".into(),
            Self::Refreshing {
                last_success_at: Some(at),
            } => format!(
                "Refreshing models; previous result is {} seconds old.",
                age(*at)
            ),
            Self::Refreshing {
                last_success_at: None,
            } => "Discovering models…".into(),
            Self::Fresh { observed_at } => {
                format!("Models discovered {} seconds ago.", age(*observed_at))
            }
            Self::Stale {
                last_success_at,
                error,
            } => format!(
                "Showing models discovered {} seconds ago. Refresh failed: {error}",
                age(*last_success_at)
            ),
            Self::Unavailable { error } => format!("Model discovery failed: {error}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelIdentity {
    pub harness: EngineId,
    pub native_id: String,
    pub gateway: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ChoiceSemantic {
    Default,
    Intensity(ThinkingLevel),
    Activation(bool),
    Special,
    Speed(SpeedLevel),
    Native,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ConfigBinding {
    Effort(Option<String>),
    Variant(Option<String>),
    Speed(Option<String>),
    Context(Option<u64>),
    Compaction(Option<u64>),
    Permission(Permissions),
}

impl ConfigBinding {
    pub fn selected(&self, config: &EngineConfig) -> bool {
        match self {
            Self::Effort(value) => config.effort == *value,
            Self::Variant(value) => config.variant == *value,
            Self::Speed(value) => config.speed == *value,
            Self::Context(value) => config.context_window_override() == *value,
            Self::Compaction(value) => config.compaction_window_override() == *value,
            Self::Permission(value) => config.permissions == *value,
        }
    }

    pub fn apply(&self, config: &EngineConfig) -> EngineConfig {
        let mut result = config.clone();
        match self {
            Self::Effort(value) => result.effort.clone_from(value),
            Self::Variant(value) => result.variant.clone_from(value),
            Self::Speed(value) => result.speed.clone_from(value),
            Self::Context(value) | Self::Compaction(value) => {
                let key = match self {
                    Self::Context(_) => "context_window",
                    Self::Compaction(_)
                    | Self::Effort(_)
                    | Self::Variant(_)
                    | Self::Speed(_)
                    | Self::Permission(_) => "compaction_window",
                };
                match value {
                    Some(value) => {
                        result.options.insert(key.into(), (*value).into());
                    }
                    None => {
                        result.options.remove(key);
                    }
                }
            }
            Self::Permission(value) => result.permissions = *value,
        }
        result
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "name", rename_all = "snake_case")]
pub enum OptionOrigin {
    Discovered,
    HarnessPolicy,
    Customization(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelChoice {
    pub id: String,
    pub label: String,
    pub semantic: ChoiceSemantic,
    pub binding: ConfigBinding,
    pub description: Option<String>,
    pub advisory: Option<String>,
    pub origin: OptionOrigin,
    pub hidden: bool,
    pub visibility_override: Option<bool>,
}

impl ModelChoice {
    pub fn apply(&self, config: &EngineConfig) -> EngineConfig {
        let mut result = self.binding.apply(config);
        match self.semantic {
            ChoiceSemantic::Intensity(level) => {
                result
                    .options
                    .insert("thinking_level".into(), serde_json::json!(level));
                result
                    .options
                    .insert("thinking_enabled".into(), true.into());
            }
            ChoiceSemantic::Activation(enabled) => {
                result
                    .options
                    .insert("thinking_enabled".into(), enabled.into());
            }
            ChoiceSemantic::Default
                if matches!(
                    self.binding,
                    ConfigBinding::Effort(_) | ConfigBinding::Variant(_)
                ) =>
            {
                result.options.remove("thinking_level");
                result.options.remove("thinking_enabled");
            }
            ChoiceSemantic::Default
            | ChoiceSemantic::Special
            | ChoiceSemantic::Speed(_)
            | ChoiceSemantic::Native => {}
        }
        result
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelControl {
    pub axis: ModelAxis,
    pub choices: Vec<ModelChoice>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelSpecification {
    pub identity: ModelIdentity,
    pub images: Capability<bool>,
    pub context_capacity: Capability<u64>,
    pub controls: Vec<ModelControl>,
    pub evidence: ModelEvidence,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomizationScope {
    pub harness: Option<EngineId>,
    pub model: Option<String>,
}

impl CustomizationScope {
    fn matches(&self, identity: &ModelIdentity) -> bool {
        self.harness
            .is_none_or(|harness| harness == identity.harness)
            && self
                .model
                .as_ref()
                .is_none_or(|model| model == &identity.native_id)
    }

    fn specificity(&self) -> u8 {
        u8::from(self.harness.is_some()) + 2 * u8::from(self.model.is_some())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCustomization {
    pub name: String,
    pub scope: CustomizationScope,
    pub axis: ModelAxis,
    pub native_id: String,
    pub label: Option<String>,
    pub semantic: Option<ChoiceSemantic>,
    pub hidden: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCustomizationSet {
    pub version: u32,
    pub rules: Vec<ModelCustomization>,
}

pub fn model_customizations(value: &serde_json::Value) -> Result<Vec<ModelCustomization>, String> {
    let set: ModelCustomizationSet =
        serde_json::from_value(value.clone()).map_err(|error| error.to_string())?;
    if set.version != 1 {
        return Err("Unsupported model customization version.".into());
    }
    validate_model_customizations(&set.rules)?;
    Ok(set.rules)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolutionChange {
    pub axis: ModelAxis,
    pub requested: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedModelConfig {
    pub requested: EngineConfig,
    pub effective: EngineConfig,
    pub changes: Vec<ResolutionChange>,
}

fn token_label(tokens: u64) -> String {
    match tokens {
        n if n >= 1_000_000 && n.is_multiple_of(1_000_000) => format!("{}M", n / 1_000_000),
        n if n.is_multiple_of(1_000) => format!("{}K", n / 1_000),
        n => n.to_string(),
    }
}

fn native_label(id: &str) -> String {
    let mut chars = id.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

fn reasoning_semantic(id: &str) -> ChoiceSemantic {
    match id {
        "none" | "off" | "disabled" => ChoiceSemantic::Activation(false),
        "thinking" | "on" | "enabled" => ChoiceSemantic::Activation(true),
        "ultra" => ChoiceSemantic::Special,
        _ => {
            ThinkingLevel::from_native(id).map_or(ChoiceSemantic::Native, ChoiceSemantic::Intensity)
        }
    }
}

fn semantic_label(semantic: &ChoiceSemantic, native: &str) -> String {
    match semantic {
        ChoiceSemantic::Intensity(level) => level.label().into(),
        ChoiceSemantic::Activation(true) => "On".into(),
        ChoiceSemantic::Activation(false) => "Off".into(),
        ChoiceSemantic::Speed(level) => level.label().into(),
        ChoiceSemantic::Default | ChoiceSemantic::Special | ChoiceSemantic::Native => native.into(),
    }
}

const fn reasoning_axis(semantic: &ChoiceSemantic) -> ModelAxis {
    match semantic {
        ChoiceSemantic::Intensity(_) => ModelAxis::Thinking,
        ChoiceSemantic::Activation(_) => ModelAxis::Activation,
        ChoiceSemantic::Special => ModelAxis::Special,
        ChoiceSemantic::Default | ChoiceSemantic::Native | ChoiceSemantic::Speed(_) => {
            ModelAxis::Variant
        }
    }
}
