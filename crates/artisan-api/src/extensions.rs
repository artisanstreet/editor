use crate::EngineId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillOrigin {
    FirstParty,
    User,
    Discovered,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillEngineStatus {
    Synced,
    Off,
    EngineAbsent,
    Unavailable,
    Conflict,
    Native,
    NotPresent,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillEngineState {
    pub engine: EngineId,
    pub status: SkillEngineStatus,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listing {
    pub id: String,
    pub name: String,
    pub publisher: String,
    pub source: String,
    pub installs: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallListing {
    pub id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogSkill {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub origin: SkillOrigin,
    pub enabled: bool,
    pub file_count: u32,
    pub byte_count: u64,
    pub engines: Vec<SkillEngineState>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptEngineStatus {
    Synced,
    Drifted,
    AwaitingChoice,
    EngineAbsent,
    Unavailable,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptEngineState {
    pub engine: EngineId,
    pub status: PromptEngineStatus,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub external_text: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptCandidate {
    pub id: String,
    pub engines: Vec<EngineId>,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlobalPrompt {
    pub version: u64,
    pub text: String,
    #[serde(default)]
    pub candidates: Vec<PromptCandidate>,
    pub engines: Vec<PromptEngineState>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavePrompt {
    pub base_version: u64,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResolvePrompt {
    Choose { candidate_id: String },
    UseEngine { engine: EngineId },
    RestoreEngine { engine: EngineId },
}

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
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
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sse: bool,
}

fn origin(url: &str) -> String {
    let (scheme, rest) = url
        .split_once("://")
        .map_or(("", url), |(scheme, rest)| (scheme, rest));
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if scheme.is_empty() {
        host.to_owned()
    } else {
        format!("{scheme}://{host}")
    }
}

impl std::fmt::Debug for McpServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpServer")
            .field("name", &self.name)
            .field("command", &self.command)
            .field("args", &self.args.len())
            .field("env", &self.env.keys().collect::<Vec<_>>())
            .field("url", &self.url.as_deref().map(origin))
            .field("headers", &self.headers.keys().collect::<Vec<_>>())
            .field("sse", &self.sse)
            .finish()
    }
}

pub const MCP_AUTH_KEY: &str = "mcp_auth";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpAuthStatus {
    NeedsSignIn,
    SignedIn,
    NeedsToken,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpSignInStart {
    pub server: String,
    pub redirect_uri: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpSignInStarted {
    pub authorize_url: String,
    pub state: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpSignInFinish {
    pub state: String,
    pub code: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpSignOut {
    pub server: String,
}

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
    Use { version: String },
    Login {},
    LoginCode { code: String },
    CancelLogin {},
    Logout {},
    Refresh {},
    Versions {},
    ClaudeAiSignIn { session: crate::ClaudeAiSession },
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
#[path = "../tests/extensions/engine_command.rs"]
mod engine_command_tests;

#[cfg(test)]
#[path = "../tests/extensions/mcp_server.rs"]
mod mcp_server_tests;
