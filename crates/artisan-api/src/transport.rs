pub const TRANSPORT_KEY: &str = "claude_transport";

pub const CLAUDE_AI_SESSION_PREFIX: &str = "claude.ai:";

pub const CLAUDE_CODE: &str = "claude_code";

pub const CLAUDE_AI: &str = "claude_ai";

#[must_use]
pub fn transport_of(value: Option<&serde_json::Value>) -> &'static str {
    match value.and_then(serde_json::Value::as_str) {
        Some(CLAUDE_AI) => CLAUDE_AI,
        _ => CLAUDE_CODE,
    }
}

#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ClaudeAiSession {
    pub cookies: Vec<(String, String)>,
    pub user_agent: String,
}

impl ClaudeAiSession {
    #[must_use]
    pub fn cookie(&self, name: &str) -> Option<&str> {
        self.cookies
            .iter()
            .find(|(cookie, _)| cookie == name)
            .map(|(_, value)| value.as_str())
    }

    #[must_use]
    pub fn signed_in(&self) -> bool {
        CLAUDE_AI_SESSION_COOKIES
            .iter()
            .any(|name| self.cookie(name).is_some_and(|value| !value.is_empty()))
    }
}

const CLAUDE_AI_SESSION_COOKIES: [&str; 2] = ["sessionKey", "sessionKeyV3"];

impl std::fmt::Debug for ClaudeAiSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClaudeAiSession")
            .field(
                "cookies",
                &self
                    .cookies
                    .iter()
                    .map(|(name, _)| name)
                    .collect::<Vec<_>>(),
            )
            .field("user_agent", &self.user_agent)
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ClaudeAiState {
    SignedOut,
    SignedIn { account: Option<String> },
    NeedsRenewal { account: Option<String> },
    Expired { reason: String },
}

#[cfg(test)]
#[path = "../tests/transport/unit.rs"]
mod tests;
