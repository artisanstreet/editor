//! Every setting an agent or the user can change, declared once: the
//! Artisan MCP's `settings_*` tools read these, and so can the settings
//! pages and `ae`. A setting lives either in Forge (shared by every Editor
//! of that host) or in the Editor (this device's `settings.json`).

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
    /// One of these values (numbers or `null`).
    Choice {
        values: &'static [Option<u32>],
    },
    /// A name; `hint` says which names fit.
    Text {
        hint: &'static str,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct SettingDecl {
    pub key: &'static str,
    pub owner: SettingOwner,
    pub kind: SettingKind,
    /// The value while unset, as JSON.
    pub default: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    /// Changing it shows an approval card first.
    pub needs_approval: bool,
}

impl SettingDecl {
    #[must_use]
    pub fn default_value(&self) -> Value {
        serde_json::from_str(self.default).unwrap_or(Value::Null)
    }

    /// Why `value` can't be this setting's, if it can't.
    ///
    /// # Errors
    ///
    /// The value's type or choice doesn't fit the declaration.
    pub fn check(&self, value: &Value) -> Result<(), String> {
        match self.kind {
            SettingKind::Bool if value.is_boolean() => Ok(()),
            SettingKind::Bool => Err(format!("{} takes true or false.", self.key)),
            SettingKind::Text { .. } if value.is_string() => Ok(()),
            SettingKind::Text { hint } => Err(format!("{} takes {hint}.", self.key)),
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
                    let listed: Vec<String> = values
                        .iter()
                        .map(|value| value.map_or_else(|| "null".to_owned(), |n| n.to_string()))
                        .collect();
                    Err(format!("{} takes one of {}.", self.key, listed.join(", ")))
                }
            }
        }
    }
}

pub const SETTINGS: &[SettingDecl] = &[
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
        description: "Name threads with the harness's generated summary. Off, a thread keeps the title from its first message.",
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
        label: "Retry when the provider is overloaded",
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
        key: "computer_use",
        owner: SettingOwner::Forge,
        kind: SettingKind::Bool,
        default: "false",
        label: "Computer use",
        description: "Let agents capture and control the configured agent display. A virtual monitor still shares your keyboard and cursor; your input pauses automation. Moving your windows, launching apps and accessing the shared clipboard require approval.",
        needs_approval: true,
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
        key: "font_set",
        owner: SettingOwner::Editor,
        kind: SettingKind::Text {
            hint: "\"spline\" (the bundled Spline Sans and Spline Sans Mono) or \"system\" (the platform's UI and monospace fonts)",
        },
        default: "\"spline\"",
        label: "Fontset",
        description: "The families the Editor draws text in: \"spline\" or \"system\".",
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
        key: "reduced_motion",
        owner: SettingOwner::Editor,
        kind: SettingKind::Bool,
        default: "false",
        label: "Reduce motion",
        description: "Turn off nonessential animation in the Editor.",
        needs_approval: false,
    },
];

/// The declaration of `key`, if it's a setting.
#[must_use]
pub fn setting_decl(key: &str) -> Option<&'static SettingDecl> {
    SETTINGS.iter().find(|decl| decl.key == key)
}

/// What Forge asks a connected Editor to do for an agent: the Artisan MCP
/// tools whose state lives in the Editor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "call", rename_all = "snake_case")]
pub enum EditorCall {
    HostsList,
    /// Pair with a Forge: `this_computer`, `wsl` (with `distro`), or
    /// `remote` (with `address`, `code` and `fingerprint` from the other
    /// machine's `ae pair`).
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
    /// Open a thread of the host Forge runs on.
    OpenThread {
        thread: crate::ThreadId,
    },
    /// Open a settings page by its link name (`general`, `hosts`, ...).
    OpenSettings {
        page: String,
    },
    /// Start signing in to one of the calling host's HTTP MCP servers:
    /// this Editor catches the browser's redirect. With `navigate_user`
    /// it also opens the sign-in page and shows the server's card.
    /// Answers `{ "authorize_url": … }`.
    McpSignIn {
        server: String,
        #[serde(default)]
        navigate_user: bool,
    },
}

/// The Editor's answer to an [`EditorCall`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditorReply {
    Ok(Value),
    Error(String),
}
