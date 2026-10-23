//! What Forge keeps in every engine besides threads: skills and the global
//! prompt, with where each stands in each engine. Routes:
//!
//! ```text
//! GET  /skills/catalog                         Vec<CatalogSkill>
//! POST /skills/{id}/enable | disable | adopt   Vec<CatalogSkill>
//! GET  /prompt                                 GlobalPrompt
//! PUT  /prompt                                 SavePrompt → GlobalPrompt (409 when stale)
//! POST /prompt/resolve                         ResolvePrompt → GlobalPrompt
//! ```

use crate::EngineId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Where a skill comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillOrigin {
    /// Ships with Artisan (the repository's `skills/`, embedded in Forge).
    FirstParty,
    /// The user's own, in `<data>/extensions/skills`.
    User,
    /// Found in an engine's skills folder without Artisan installing it:
    /// Forge quarantines it, out of every engine, until the user installs
    /// (adopts) or discards it.
    Discovered,
}

/// Where one skill stands in one engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillEngineStatus {
    /// Forge's copy is in the engine's skills folder.
    Synced,
    /// The skill is turned off, so Forge keeps it out of the engine.
    Off,
    /// The engine isn't installed; the skill follows once it is.
    EngineAbsent,
    /// The engine reads no skills folder Forge can manage.
    Unavailable,
    /// The engine has its own, different skill under the same name.
    Conflict,
    /// A discovered skill that came from this engine.
    Native,
    /// A discovered skill this engine doesn't have.
    NotPresent,
    /// Placing the skill failed; `reason` says why.
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillEngineState {
    pub engine: EngineId,
    pub status: SkillEngineStatus,
    /// Why, for `Unavailable`, `Conflict` and `Failed`.
    #[serde(default)]
    pub reason: Option<String>,
}

/// A skill another directory lists, found by the Marketplace's search.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listing {
    /// `skills.sh:<owner>/<repo>/<skill>`; what [`InstallListing`] names.
    pub id: String,
    pub name: String,
    /// The GitHub repository that publishes it, `<owner>/<repo>`.
    pub publisher: String,
    /// The directory that listed it, `skills.sh`.
    pub source: String,
    pub installs: u64,
}

/// Install a [`Listing`] into Artisan's own skills.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallListing {
    pub id: String,
}

/// One skill and its state in every engine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogSkill {
    /// `artisan.<name>`, `local.<folder>` or `discovered.<name>.<digest>`.
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub origin: SkillOrigin,
    /// Whether Forge keeps it in the engines; always `false` for a
    /// discovered skill.
    pub enabled: bool,
    pub file_count: u32,
    pub byte_count: u64,
    /// One entry per engine, in [`EngineId::ALL`] order.
    pub engines: Vec<SkillEngineState>,
}

/// Where the global prompt stands in one engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptEngineStatus {
    /// The engine's global file holds the prompt.
    Synced,
    /// The file was edited outside Artisan; `external_text` holds what it
    /// says now. Forge leaves it alone until the user decides.
    Drifted,
    /// The engines hold different prompts; waiting for the user's choice.
    AwaitingChoice,
    EngineAbsent,
    /// The engine reads no global prompt file.
    Unavailable,
    /// Reading or writing the file failed; `reason` says why.
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptEngineState {
    pub engine: EngineId,
    pub status: PromptEngineStatus,
    /// The engine's global file, relative to its home (`.claude/CLAUDE.md`).
    #[serde(default)]
    pub file: Option<String>,
    /// What the file says now, for `Drifted`.
    #[serde(default)]
    pub external_text: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
}

/// One distinct prompt found in the engines before Forge took them over.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptCandidate {
    /// SHA-256 of the text.
    pub id: String,
    pub engines: Vec<EngineId>,
    pub text: String,
}

/// The global prompt and its state in every engine.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlobalPrompt {
    /// The saved version; every save moves it forward. `0` before the first
    /// save through Artisan.
    pub version: u64,
    pub text: String,
    /// Non-empty while the engines hold different prompts and the user must
    /// choose which one Forge keeps; nothing is written until then.
    #[serde(default)]
    pub candidates: Vec<PromptCandidate>,
    /// One entry per engine, in [`EngineId::ALL`] order.
    pub engines: Vec<PromptEngineState>,
}

/// Saves a new version. Refused (409) when `base_version` isn't the latest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavePrompt {
    pub base_version: u64,
    pub text: String,
}

/// Settles the setup choice or an engine edited outside Artisan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResolvePrompt {
    /// Keep this candidate in every engine.
    Choose { candidate_id: String },
    /// Make the engine's edited text the prompt everywhere.
    UseEngine { engine: EngineId },
    /// Write Artisan's prompt over the engine's edited file.
    RestoreEngine { engine: EngineId },
}

/// A stdio server (`command`, `args`, `env`) or, with `url`, an HTTP one.
/// Values may hold `${VAR}` and `${VAR:-default}`, expanded from the
/// environment when the server starts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServer {
    pub name: String,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// HTTP headers sent with every request to a `url` server (an
    /// `Authorization` token, an API key).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    /// With `url`: the legacy HTTP+SSE transport rather than Streamable HTTP.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sse: bool,
}

/// Forge's setting with the sign-in state of each HTTP MCP server that has
/// one, by server name (a map of [`McpAuthStatus`]). Forge writes it.
pub const MCP_AUTH_KEY: &str = "mcp_auth";

/// Where an HTTP MCP server stands on signing in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpAuthStatus {
    /// It answers 401 and offers OAuth sign-in.
    NeedsSignIn,
    /// Artisan holds a token it accepts.
    SignedIn,
    /// It answers 401 but offers no OAuth sign-in: it needs a token in an
    /// `Authorization` header.
    NeedsToken,
}

/// Starts signing in an HTTP MCP server. `redirect_uri` is the loopback
/// address where the Editor catches the browser's redirect.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpSignInStart {
    pub server: String,
    pub redirect_uri: String,
}

/// The page the user approves the sign-in on, and the `state` its redirect
/// carries back.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpSignInStarted {
    pub authorize_url: String,
    pub state: String,
}

/// The authorization code the redirect brought back, for Forge to exchange.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpSignInFinish {
    pub state: String,
    pub code: String,
}

/// Forgets the tokens Artisan holds for a server.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpSignOut {
    pub server: String,
}

/// Edits one server against its original value; concurrent unrelated edits survive.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum McpMutation {
    Upsert {
        previous: Option<McpServer>,
        server: McpServer,
    },
    Remove {
        previous: McpServer,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "kebab-case", deny_unknown_fields)]
pub enum EngineCommand {
    Install {},
    Update {},
    Rollback {},
    Use {
        version: String,
    },
    Login {},
    LoginCode {
        code: String,
    },
    CancelLogin {},
    Logout {},
    Refresh {},
    Versions {},
    /// Keep a claude.ai session a browser signed in to (Claude only).
    ClaudeAiSignIn {
        session: crate::ClaudeAiSession,
    },
    /// Forget the claude.ai session (Claude only).
    ClaudeAiSignOut {},
}
impl EngineCommand {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Install {} => "install",
            Self::Update {} => "update",
            Self::Rollback {} => "rollback",
            Self::Use { .. } => "use",
            Self::Login {} => "login",
            Self::LoginCode { .. } => "login-code",
            Self::CancelLogin {} => "cancel-login",
            Self::Logout {} => "logout",
            Self::Refresh {} => "refresh",
            Self::Versions {} => "versions",
            Self::ClaudeAiSignIn { .. } => "claude-ai-sign-in",
            Self::ClaudeAiSignOut {} => "claude-ai-sign-out",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillAction {
    Enable,
    Disable,
    Adopt,
    /// Delete a quarantined skill.
    Discard,
}
impl SkillAction {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Enable => "enable",
            Self::Disable => "disable",
            Self::Adopt => "adopt",
            Self::Discard => "discard",
        }
    }
}

#[cfg(test)]
mod engine_command_tests {
    use super::EngineCommand;
    #[test]
    fn command_arguments_are_required_and_unrelated_arguments_are_rejected() {
        assert!(serde_json::from_str::<EngineCommand>(r#"{"command":"use"}"#).is_err());
        assert!(serde_json::from_str::<EngineCommand>(r#"{"command":"login-code"}"#).is_err());
        assert!(
            serde_json::from_str::<EngineCommand>(r#"{"command":"install","version":"1"}"#)
                .is_err()
        );
        let command = EngineCommand::Use {
            version: "1.2.3".into(),
        };
        assert_eq!(
            serde_json::from_str::<EngineCommand>(&serde_json::to_string(&command).unwrap())
                .unwrap(),
            command
        );
    }
}
