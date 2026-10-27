//! The settings the Claude engine's transport is chosen by.
//!
//! The transport is a property of the account, not of a thread: every
//! Claude thread on a Forge follows the same one. `claude_code` is the
//! official `claude -p` way; `claude_ai` speaks Anthropic's chat backend
//! directly, which is the only place the provider's one-line recap of the
//! model's thinking exists.

/// The setting that names the transport.
pub const TRANSPORT_KEY: &str = "claude_transport";

/// The prefix a claude.ai session carries, so a run that used it keeps
/// using it. A session without one came from Claude Code.
pub const CLAUDE_AI_SESSION_PREFIX: &str = "claude.ai:";

/// The official `claude -p` way: Claude Code runs the turn.
pub const CLAUDE_CODE: &str = "claude_code";

/// Anthropic's chat backend, spoken to directly.
pub const CLAUDE_AI: &str = "claude_ai";

/// The transport a setting names, and the one to use when it says nothing.
#[must_use]
pub fn transport_of(value: Option<&serde_json::Value>) -> &'static str {
    match value.and_then(serde_json::Value::as_str) {
        Some(CLAUDE_AI) => CLAUDE_AI,
        _ => CLAUDE_CODE,
    }
}

/// The transport a session was made with, when it says.
///
/// A run carries on the path its session was made on: a Claude Code session
/// stays on Claude Code, and a claude.ai conversation stays on claude.ai,
/// whatever the setting says since.
#[must_use]
pub fn transport_of_session(session: Option<&str>) -> Option<&'static str> {
    let session = session?;
    if session.starts_with(CLAUDE_AI_SESSION_PREFIX) {
        Some(CLAUDE_AI)
    } else if session.is_empty() {
        None
    } else {
        Some(CLAUDE_CODE)
    }
}

/// A claude.ai session a browser signed in to: what Forge needs to speak
/// claude.ai's chat API as the user. It is captured on the user's own
/// device, where the browser is, and handed to the Forge.
#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ClaudeAiSession {
    /// claude.ai's cookies, name and value.
    pub cookies: Vec<(String, String)>,
    /// The browser's user agent. Cloudflare's clearance is bound to it.
    pub user_agent: String,
}

impl ClaudeAiSession {
    /// The cookie with this name, if the session carries it.
    #[must_use]
    pub fn cookie(&self, name: &str) -> Option<&str> {
        self.cookies
            .iter()
            .find(|(cookie, _)| cookie == name)
            .map(|(_, value)| value.as_str())
    }

    /// Whether the cookies hold a signed-in claude.ai session.
    #[must_use]
    pub fn signed_in(&self) -> bool {
        CLAUDE_AI_SESSION_COOKIES
            .iter()
            .any(|name| self.cookie(name).is_some_and(|value| !value.is_empty()))
    }
}

/// The cookies that carry a signed-in claude.ai session.
pub const CLAUDE_AI_SESSION_COOKIES: [&str; 2] = ["sessionKey", "sessionKeyV3"];

/// The cookies are credentials: a debug print names them, never their values.
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

/// Where the claude.ai session claude.ai mode runs on stands.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ClaudeAiState {
    /// No session: Claude's turns run on Claude Code.
    SignedOut,
    SignedIn {
        account: Option<String>,
    },
    /// claude.ai asked for a browser check. The device that signed in
    /// renews the session by itself; until then turns run on Claude Code.
    NeedsRenewal {
        account: Option<String>,
    },
    /// claude.ai no longer takes the session: sign in again.
    Expired {
        reason: String,
    },
}

impl ClaudeAiState {
    /// Whether a turn can run on claude.ai now.
    #[must_use]
    pub const fn usable(&self) -> bool {
        matches!(self, Self::SignedIn { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_unset_or_unknown_setting_means_claude_code() {
        assert_eq!(transport_of(None), CLAUDE_CODE);
        assert_eq!(transport_of(Some(&json!("claude_code"))), CLAUDE_CODE);
        assert_eq!(transport_of(Some(&json!("something else"))), CLAUDE_CODE);
        assert_eq!(transport_of(Some(&json!(true))), CLAUDE_CODE);
    }

    #[test]
    fn the_claude_ai_transport_is_named_by_its_setting() {
        assert_eq!(transport_of(Some(&json!(CLAUDE_AI))), CLAUDE_AI);
    }

    #[test]
    fn a_session_carries_the_path_it_was_made_on() {
        assert_eq!(
            transport_of_session(Some("claude.ai:1f0c9e2a")),
            Some(CLAUDE_AI)
        );
        assert_eq!(
            transport_of_session(Some("b301e08e-3ad7-47b8-b4d7")),
            Some(CLAUDE_CODE)
        );
        assert_eq!(transport_of_session(None), None);
        assert_eq!(transport_of_session(Some("")), None);
    }
}
