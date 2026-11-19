use std::path::{Path, PathBuf};
use std::sync::Mutex;

use artisan_api::{ClaudeAiSession, ClaudeAiState};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum LoadSessionError {
    #[error("reading the claude.ai session {path}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path} is not a claude.ai session")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum SaveSessionError {
    #[error("encoding the claude.ai session for {path}")]
    Encode {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("writing the claude.ai session {path}")]
    Write {
        path: PathBuf,
        #[source]
        source: artisan_paths::private::PrivateFileError,
    },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum UpdateSessionError {
    #[error("reading the claude.ai session to update it")]
    Load(#[source] LoadSessionError),
    #[error("writing the updated claude.ai session")]
    Save(#[source] SaveSessionError),
}

#[derive(Debug, thiserror::Error)]
#[error("removing the claude.ai session {path}")]
pub struct RemoveSessionError {
    path: PathBuf,
    #[source]
    source: std::io::Error,
}

static FILE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub(crate) cookies: Vec<(String, String)>,
    pub(crate) user_agent: String,
    pub(crate) org_uuid: String,
    #[serde(default)]
    pub(crate) account: Option<String>,
    #[serde(default)]
    pub(crate) needs_renewal: bool,
    #[serde(default)]
    pub(crate) expired: Option<String>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field(
                "cookies",
                &self
                    .cookies
                    .iter()
                    .map(|(name, _)| name)
                    .collect::<Vec<_>>(),
            )
            .field("org_uuid", &self.org_uuid)
            .field("account", &self.account)
            .field("needs_renewal", &self.needs_renewal)
            .field("expired", &self.expired)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CookieChange {
    Set(String, String),
    Remove(String),
}

impl Session {
    #[must_use]
    pub fn new(captured: ClaudeAiSession, org_uuid: String, account: Option<String>) -> Self {
        Self {
            cookies: captured.cookies,
            user_agent: captured.user_agent,
            org_uuid,
            account,
            needs_renewal: false,
            expired: None,
        }
    }

    #[must_use]
    pub(crate) fn cookie_header(&self) -> String {
        self.cookies
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; ")
    }

    #[must_use]
    pub fn path(home: &Path) -> PathBuf {
        home.join(".claude-ai").join("session.json")
    }

    #[must_use]
    pub fn state(home: &Path) -> ClaudeAiState {
        match Self::load(home) {
            Ok(Some(session)) => session.status(),
            Ok(None) => ClaudeAiState::SignedOut,
            Err(error) => ClaudeAiState::Expired {
                reason: format!("the session could not be read: {error:#}"),
            },
        }
    }

    #[must_use]
    pub(crate) fn status(&self) -> ClaudeAiState {
        let account = self.account.clone();
        self.expired.clone().map_or_else(
            || {
                if self.needs_renewal {
                    ClaudeAiState::NeedsRenewal { account }
                } else {
                    ClaudeAiState::SignedIn { account }
                }
            },
            |reason| ClaudeAiState::Expired { reason },
        )
    }

    pub(crate) fn load(home: &Path) -> Result<Option<Self>, LoadSessionError> {
        let path = Self::path(home);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(LoadSessionError::Read { path, source }),
        };
        serde_json::from_str(&text)
            .map(Some)
            .map_err(|source| LoadSessionError::Parse { path, source })
    }

    pub fn save(&self, home: &Path) -> Result<(), SaveSessionError> {
        let _guard = FILE_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.write(home)
    }

    fn write(&self, home: &Path) -> Result<(), SaveSessionError> {
        let path = Self::path(home);
        let text =
            serde_json::to_string_pretty(self).map_err(|source| SaveSessionError::Encode {
                path: path.clone(),
                source,
            })?;
        artisan_paths::private::write_file(&path, text.as_bytes())
            .map_err(|source| SaveSessionError::Write { path, source })
    }

    pub(crate) fn update(home: &Path, change: impl FnOnce(&mut Self)) -> Result<(), UpdateSessionError> {
        let _guard = FILE_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(mut session) = Self::load(home).map_err(UpdateSessionError::Load)? else {
            return Ok(());
        };
        let before = session.clone();
        change(&mut session);
        if session == before {
            return Ok(());
        }
        session.write(home).map_err(UpdateSessionError::Save)
    }

    pub(crate) fn remove(home: &Path) -> Result<(), RemoveSessionError> {
        let _guard = FILE_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let path = Self::path(home);
        match std::fs::remove_file(&path) {
            Err(source) if source.kind() != std::io::ErrorKind::NotFound => {
                Err(RemoveSessionError { path, source })
            }
            _ => Ok(()),
        }
    }

    pub(crate) fn apply(&mut self, changes: &[CookieChange]) {
        for change in changes {
            match change {
                CookieChange::Set(name, value) => {
                    if let Some(slot) = self.cookies.iter_mut().find(|(cookie, _)| cookie == name) {
                        slot.1.clone_from(value);
                    } else {
                        self.cookies.push((name.clone(), value.clone()));
                    }
                }
                CookieChange::Remove(name) => self.cookies.retain(|(cookie, _)| cookie != name),
            }
        }
    }
}

#[must_use]
pub(super) fn cookie_change(header: &str, now: chrono::DateTime<chrono::Utc>) -> Option<CookieChange> {
    let mut parts = header.split(';');
    let (name, value) = parts.next()?.trim().split_once('=')?;
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let mut removed = false;
    for attribute in parts {
        let (key, argument) = attribute.trim().split_once('=').map_or_else(
            || (attribute.trim(), ""),
            |(key, value)| (key.trim(), value.trim()),
        );
        match key.to_ascii_lowercase().as_str() {
            "domain" => {
                let domain = argument.trim_start_matches('.').to_ascii_lowercase();
                if domain != "claude.ai" && !domain.ends_with(".claude.ai") {
                    return None;
                }
            }
            "max-age" => removed |= argument.parse::<i64>().is_ok_and(|age| age <= 0),
            "expires" => {
                removed |= chrono::DateTime::parse_from_rfc2822(argument)
                    .is_ok_and(|expires| expires < now);
            }
            _ => {}
        }
    }
    Some(if removed {
        CookieChange::Remove(name.to_owned())
    } else {
        CookieChange::Set(name.to_owned(), value.trim().to_owned())
    })
}

#[cfg(test)]
#[path = "../../tests/claudeai/session.rs"]
mod tests;
