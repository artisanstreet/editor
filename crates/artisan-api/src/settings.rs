use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingOwner {
    Forge,
    Editor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SettingKind {
    Bool,
    SystemSwitch,
    ModelCustomizations,
    Choice { values: &'static [Option<u32>] },
    Options { values: &'static [&'static str] },
    Text { hint: &'static str },
}

#[derive(Debug, thiserror::Error)]
pub enum SettingValueError {
    #[error("{key} takes true or false")]
    NotBool { key: &'static str },
    #[error("{key} takes true, false or null to follow the system")]
    NotSystemSwitch { key: &'static str },
    #[error("{key} takes {hint}")]
    NotText {
        key: &'static str,
        hint: &'static str,
    },
    #[error("{key} takes one of {}", listed(values))]
    NotAChoice {
        key: &'static str,
        values: &'static [Option<u32>],
    },
    #[error("{key} takes one of {}", values.join(", "))]
    NotAnOption {
        key: &'static str,
        values: &'static [&'static str],
    },
    #[error("{key} takes a model customization set")]
    ModelCustomizations {
        key: &'static str,
        #[source]
        source: crate::ModelCustomizationsError,
    },
}

fn listed(values: &[Option<u32>]) -> String {
    values
        .iter()
        .map(|value| value.map_or_else(|| "null".to_owned(), |n| n.to_string()))
        .collect::<Vec<_>>()
        .join(", ")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct SettingDecl {
    pub key: &'static str,
    pub owner: SettingOwner,
    pub kind: SettingKind,
    pub default: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub needs_approval: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Switch {
    Stored(bool),
    Unset(bool),
    Damaged(bool),
}

impl Switch {
    #[must_use]
    pub const fn on(self) -> bool {
        match self {
            Self::Stored(on) | Self::Unset(on) | Self::Damaged(on) => on,
        }
    }
}

impl SettingDecl {
    #[must_use]
    pub fn switch(&self, stored: Option<&Value>) -> Switch {
        match stored {
            Some(Value::Bool(on)) => Switch::Stored(*on),
            None => Switch::Unset(self.declared_switch()),
            Some(_) => Switch::Damaged(self.declared_switch()),
        }
    }

    fn declared_switch(&self) -> bool {
        self.default_value().as_bool().unwrap_or_else(|| {
            unreachable!(
                "invariant: setting {} is read as a switch, so it declares an on or off default",
                self.key
            )
        })
    }

    #[must_use]
    pub fn default_value(&self) -> Value {
        serde_json::from_str(self.default).unwrap_or_else(|error| {
            unreachable!(
                "invariant: setting {} declares a JSON default: {error}",
                self.key
            )
        })
    }

    pub fn check(&self, value: &Value) -> Result<(), SettingValueError> {
        let key = self.key;
        match self.kind {
            SettingKind::ModelCustomizations => crate::model_customizations(value)
                .map(|_| ())
                .map_err(|source| SettingValueError::ModelCustomizations { key, source }),
            SettingKind::Bool if value.is_boolean() => Ok(()),
            SettingKind::Bool => Err(SettingValueError::NotBool { key }),
            SettingKind::SystemSwitch if value.is_boolean() || value.is_null() => Ok(()),
            SettingKind::SystemSwitch => Err(SettingValueError::NotSystemSwitch { key }),
            SettingKind::Text { .. } if value.is_string() => Ok(()),
            SettingKind::Text { hint } => Err(SettingValueError::NotText { key, hint }),
            SettingKind::Options { values } => match value.as_str() {
                Some(given) if values.contains(&given) => Ok(()),
                Some(_) | None => Err(SettingValueError::NotAnOption { key, values }),
            },
            SettingKind::Choice { values } => {
                let given = match value {
                    Value::Null => Some(None),
                    Value::Number(number) => number
                        .as_u64()
                        .and_then(|number| u32::try_from(number).ok())
                        .map(Some),
                    Value::Bool(_) | Value::String(_) | Value::Array(_) | Value::Object(_) => None,
                };
                if given.is_some_and(|given| values.contains(&given)) {
                    Ok(())
                } else {
                    Err(SettingValueError::NotAChoice { key, values })
                }
            }
        }
    }
}

pub const THREAD_RETENTION_DAYS_KEY: &str = "thread_retention_days";

pub const SHOW_ALL_MODELS_KEY: &str = "show_all_models";

pub const SETTINGS: &[SettingDecl] = &[
    SettingDecl {
        key: crate::HIDE_BAD_MODEL_OPTIONS_KEY,
        owner: SettingOwner::Editor,
        kind: SettingKind::Bool,
        default: "true",
        label: "Hide bad model options",
        description: "Hide Max and Ultra for Claude and Codex, plus Light and Extra High for Opus 5.5. Existing selections remain visible.",
        needs_approval: false,
    },
    SettingDecl {
        key: crate::MODEL_CUSTOMIZATIONS_KEY,
        owner: SettingOwner::Forge,
        kind: SettingKind::ModelCustomizations,
        default: "{\"version\":1,\"rules\":[]}",
        label: "Model customizations",
        description: "Scoped labels, normalized semantics and visibility for discovered native model options.",
        needs_approval: false,
    },
    SettingDecl {
        key: "suggestions",
        owner: SettingOwner::Forge,
        kind: SettingKind::Bool,
        default: "true",
        label: "Suggest the next message",
        description: "After each turn, guess the user's next message and offer it in the composer.",
        needs_approval: false,
    },
    SettingDecl {
        key: "you_should_know",
        owner: SettingOwner::Forge,
        kind: SettingKind::Bool,
        default: "true",
        label: "You should know",
        description: "Once a thread has done enough work, look for something the user might miss and note it.",
        needs_approval: false,
    },
    SettingDecl {
        key: "summary_titles",
        owner: SettingOwner::Forge,
        kind: SettingKind::Bool,
        default: "true",
        label: "Summary titles",
        description: "Name threads with the engine's generated summary. Off, a thread keeps the title from its first message.",
        needs_approval: false,
    },
    SettingDecl {
        key: "continue_after_restart",
        owner: SettingOwner::Forge,
        kind: SettingKind::Bool,
        default: "true",
        label: "Continue after a restart",
        description: "When Forge restarts (an update, a crash) while threads are working, carry on their turns where they stopped.",
        needs_approval: false,
    },
    SettingDecl {
        key: "retry_when_overloaded",
        owner: SettingOwner::Forge,
        kind: SettingKind::Bool,
        default: "true",
        label: "Keep retrying when the provider is overloaded",
        description: "When a turn fails because the provider is at capacity or briefly unavailable (overloaded, 5xx), park the thread and keep retrying it, waiting longer each time, until it goes through.",
        needs_approval: false,
    },
    SettingDecl {
        key: "continue_after_usage_limit",
        owner: SettingOwner::Forge,
        kind: SettingKind::Bool,
        default: "true",
        label: "Continue when the usage limit resets",
        description: "When a turn stops because the account ran out of usage, park the thread and carry its turn on once the limit resets.",
        needs_approval: false,
    },
    SettingDecl {
        key: THREAD_RETENTION_DAYS_KEY,
        owner: SettingOwner::Forge,
        kind: SettingKind::Choice {
            values: &[None, Some(7), Some(14), Some(30), Some(90), Some(365)],
        },
        default: "30",
        label: "Delete inactive threads",
        description: "Delete a thread, its messages and its images once nothing has happened in it for this many days; null keeps every thread. Working, waiting and parked threads, and threads whose agents or handoffs are still active, are kept.",
        needs_approval: true,
    },
    SettingDecl {
        key: "computer_use",
        owner: SettingOwner::Forge,
        kind: SettingKind::Bool,
        default: "false",
        label: "Computer use",
        description: "Let agents capture and control the configured agent display. A virtual monitor still shares your keyboard and cursor; your input pauses automation. Moving your windows, launching apps and accessing the shared clipboard require approval.",
        needs_approval: true,
    },
    SettingDecl {
        key: SHOW_ALL_MODELS_KEY,
        owner: SettingOwner::Editor,
        kind: SettingKind::Bool,
        default: "false",
        label: "Show all models",
        description: "List every Claude and Codex model in the model picker, not only the latest of each class.",
        needs_approval: false,
    },
    SettingDecl {
        key: "notifications",
        owner: SettingOwner::Editor,
        kind: SettingKind::Bool,
        default: "true",
        label: "Notifications",
        description: "Notify when a thread finishes or needs the user while the Editor is in the background.",
        needs_approval: false,
    },
    SettingDecl {
        key: "lower_refresh_rate",
        owner: SettingOwner::Editor,
        kind: SettingKind::Bool,
        default: "false",
        label: "Lower refresh rate",
        description: "Cap the Editor's redraws at 60 frames per second; off, it matches the monitor's refresh rate.",
        needs_approval: false,
    },
    SettingDecl {
        key: "dark_mode",
        owner: SettingOwner::Editor,
        kind: SettingKind::Bool,
        default: "true",
        label: "Dark mode",
        description: "Draw the Editor in its dark theme; off draws the light theme.",
        needs_approval: false,
    },
    SettingDecl {
        key: "browser",
        owner: SettingOwner::Editor,
        kind: SettingKind::Text {
            hint: "the name of an installed browser, such as \"Helium\" or \"Google Chrome\"; empty for the system default",
        },
        default: "\"\"",
        label: "Browser",
        description: "The browser \"Search with\" opens for selected text; empty uses the system default browser.",
        needs_approval: false,
    },
    SettingDecl {
        key: "font_set",
        owner: SettingOwner::Editor,
        kind: SettingKind::Options {
            values: &["spline", "system"],
        },
        default: "\"spline\"",
        label: "Fonts",
        description: "The families the Editor draws text in: \"spline\" (the bundled Spline Sans and Spline Sans Mono) or \"system\" (the platform's UI and monospace fonts).",
        needs_approval: false,
    },
    SettingDecl {
        key: "outline_text",
        owner: SettingOwner::Editor,
        kind: SettingKind::Bool,
        default: "true",
        label: "Outline text rendering",
        description: "Rasterize glyphs from their unhinted outlines with grayscale antialiasing and blend them without gamma or contrast correction; off uses hinted, subpixel-antialiased glyphs.",
        needs_approval: false,
    },
    SettingDecl {
        key: "streaming_mode",
        owner: SettingOwner::Editor,
        kind: SettingKind::Options {
            values: &["smooth", "swift"],
        },
        default: "\"smooth\"",
        label: "Text Streaming",
        description: "How a reply appears while it streams: \"smooth\" fades new text in briefly as it arrives, \"swift\" shows it immediately without reveal animation.",
        needs_approval: false,
    },
    SettingDecl {
        key: "graphics_vsync",
        owner: SettingOwner::Editor,
        kind: SettingKind::Bool,
        default: "true",
        label: "Match monitor refresh rate",
        description: "Synchronize presentation to the current monitor; with this and the lower refresh rate both off, rendering is uncapped.",
        needs_approval: false,
    },
    SettingDecl {
        key: "reduced_motion",
        owner: SettingOwner::Editor,
        kind: SettingKind::SystemSwitch,
        default: "null",
        label: "Reduce motion",
        description: "Turn off nonessential animation in the Editor; null follows the system's reduced motion setting.",
        needs_approval: false,
    },
    SettingDecl {
        key: "high_contrast",
        owner: SettingOwner::Editor,
        kind: SettingKind::SystemSwitch,
        default: "null",
        label: "High contrast",
        description: "Draw text, borders and focus rings with stronger contrast; null follows the system's contrast setting.",
        needs_approval: false,
    },
    SettingDecl {
        key: "text_scale",
        owner: SettingOwner::Editor,
        kind: SettingKind::Choice {
            values: &[
                None,
                Some(100),
                Some(125),
                Some(150),
                Some(175),
                Some(200),
                Some(225),
            ],
        },
        default: "null",
        label: "Text size",
        description: "How large the Editor draws its text, in percent; null follows the system text size.",
        needs_approval: false,
    },
];

#[must_use]
pub fn switch_decl(key: &str) -> &'static SettingDecl {
    setting_decl(key)
        .filter(|decl| matches!(decl.kind, SettingKind::Bool))
        .unwrap_or_else(|| unreachable!("invariant: {key} is a declared on or off setting"))
}

#[must_use]
pub fn choice_values(key: &str) -> &'static [Option<u32>] {
    match setting_decl(key).map(|decl| decl.kind) {
        Some(SettingKind::Choice { values }) => values,
        Some(
            SettingKind::Bool
            | SettingKind::SystemSwitch
            | SettingKind::ModelCustomizations
            | SettingKind::Options { .. }
            | SettingKind::Text { .. },
        )
        | None => unreachable!("invariant: {key} is a declared choice setting"),
    }
}

#[must_use]
pub fn setting_decl(key: &str) -> Option<&'static SettingDecl> {
    SETTINGS.iter().find(|decl| decl.key == key)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingSaved {
    pub revision: crate::Seq,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "call", rename_all = "snake_case")]
pub enum EditorCall {
    HostsList,
    HostAdd {
        kind: String,
        #[serde(default)]
        distro: Option<String>,
        #[serde(default)]
        address: Option<String>,
        #[serde(default)]
        code: Option<String>,
        #[serde(default)]
        fingerprint: Option<String>,
    },
    HostRemove {
        host: String,
    },
    HostSwitch {
        host: String,
    },
    SettingGet {
        key: String,
    },
    SettingSet {
        key: String,
        value: Value,
    },
    OpenThread {
        thread: crate::ThreadId,
    },
    OpenSettings {
        page: String,
    },
    McpSignIn {
        server: String,
        #[serde(default)]
        navigate_user: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditorReply {
    Ok(Value),
    Error(String),
}
