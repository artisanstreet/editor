//! The claude.ai session Forge keeps.
//!
//! claude.ai's chat API is cookie-authenticated: a signed-in browser's
//! `sessionKey`, and the Cloudflare cookies bound to the browser's user
//! agent. The OAuth bearer that Claude Code holds only reaches
//! `api.anthropic.com`, which never returns the provider's recap.
//!
//! A browser on the user's device captures the session (`claude-ai-browser`)
//! and hands it to Forge, which checks it and keeps it here, readable by
//! the Forge's user only. claude.ai rotates its cookies as it answers, so
//! every response's `Set-Cookie` is written back; when Cloudflare asks for a
//! fresh browser check, the session is marked for renewal and the device
//! that signed in renews it.

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
pub enum UpdateSessionError {
    #[error("reading the claude.ai session to update it")]
    Load(#[source] LoadSessionError),
    #[error("writing the updated claude.ai session")]
    Save(#[source] SaveSessionError),
}

#[derive(Debug, thiserror::Error)]
#[error("removing the claude.ai session {path}")]
pub struct RemoveSessionError {
    pub path: PathBuf,
    #[source]
    pub source: std::io::Error,
}

/// Serialises every read-modify-write of a session file in this process:
/// runs on several threads write their cookies back at once.
static FILE_LOCK: Mutex<()> = Mutex::new(());

/// The session as Forge keeps it.
#[derive(Clone, Serialize, Deserialize)]
pub struct Session {
    /// claude.ai's cookies, name and value.
    pub cookies: Vec<(String, String)>,
    /// The browser's user agent. Cloudflare's clearance is bound to it.
    pub user_agent: String,
    /// The organization the session is signed in to.
    pub org_uuid: String,
    /// The account, as claude.ai named it when the session was checked.
    #[serde(default)]
    pub account: Option<String>,
    /// claude.ai asked for a browser check; the session works again once a
    /// browser renews it.
    #[serde(default)]
    pub needs_renewal: bool,
    /// claude.ai no longer takes the session, and why.
    #[serde(default)]
    pub expired: Option<String>,
}

/// The cookies are credentials: a debug print names them, never their values.
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

/// A cookie claude.ai set or cleared in a response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CookieChange {
    Set(String, String),
    Remove(String),
}

impl Session {
    /// A session from what a browser captured, for the organization and
    /// account claude.ai named when it was checked.
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

    /// The `Cookie` header for a request.
    #[must_use]
    pub fn cookie_header(&self) -> String {
        self.cookies
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// The cookie with this name, if the session carries it.
    #[must_use]
    pub fn cookie(&self, name: &str) -> Option<&str> {
        self.cookies
            .iter()
            .find(|(cookie, _)| cookie == name)
            .map(|(_, value)| value.as_str())
    }

    /// Where the session lives inside an engine home.
    #[must_use]
    pub fn path(home: &Path) -> PathBuf {
        home.join(".claude-ai").join("session.json")
    }

    /// What the Editor shows about the session in `home`.
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

    /// What this session's state is.
    #[must_use]
    pub fn status(&self) -> ClaudeAiState {
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

    /// Reads the session from an engine home; `None` when there is none.
    ///
    /// # Errors
    ///
    /// Returns the error if the file is there but unreadable.
    pub fn load(home: &Path) -> Result<Option<Self>, LoadSessionError> {
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

    /// Writes the session into an engine home, readable by this user only.
    ///
    /// # Errors
    ///
    /// Returns the error if the directory or file cannot be written.
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

    /// Changes the session on disk: reads what is there now, so another
    /// run's cookies are not lost, applies `change`, and writes it back.
    /// Nothing happens when there is no session (the user signed out).
    ///
    /// # Errors
    ///
    /// Returns the error if the file cannot be read or written.
    pub fn update(home: &Path, change: impl FnOnce(&mut Self)) -> Result<(), UpdateSessionError> {
        let _guard = FILE_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(mut session) = Self::load(home).map_err(UpdateSessionError::Load)? else {
            return Ok(());
        };
        change(&mut session);
        session.write(home).map_err(UpdateSessionError::Save)
    }

    /// Forgets the session.
    ///
    /// # Errors
    ///
    /// Returns the error if the file is there but cannot be removed.
    pub fn remove(home: &Path) -> Result<(), RemoveSessionError> {
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

    /// Applies the cookies a response set or cleared.
    pub fn apply(&mut self, changes: &[CookieChange]) {
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

/// What one `Set-Cookie` header does to the session, when it is claude.ai's.
#[must_use]
pub fn cookie_change(header: &str, now: chrono::DateTime<chrono::Utc>) -> Option<CookieChange> {
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
mod tests {
    use super::*;

    fn session() -> Session {
        Session::new(
            ClaudeAiSession {
                cookies: vec![
                    ("sessionKey".to_owned(), "abc".to_owned()),
                    ("__cf_bm".to_owned(), "old".to_owned()),
                ],
                user_agent: "agent".to_owned(),
            },
            "org".to_owned(),
            Some("me@example.com".to_owned()),
        )
    }

    #[test]
    fn a_session_round_trips_and_only_its_user_can_read_it() {
        let home = tempfile::tempdir().expect("a home");
        session().save(home.path()).expect("the session saves");
        let read = Session::load(home.path())
            .expect("the session loads")
            .expect("there is a session");
        assert_eq!(read.cookie("sessionKey"), Some("abc"));
        assert_eq!(
            Session::state(home.path()),
            ClaudeAiState::SignedIn {
                account: Some("me@example.com".to_owned())
            }
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(Session::path(home.path()))
                .expect("the file")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "the session is the user's alone");
        }
    }

    #[test]
    fn a_response_s_cookies_are_written_back() {
        let home = tempfile::tempdir().expect("a home");
        session().save(home.path()).expect("the session saves");
        let now = chrono::DateTime::from_timestamp_millis(artisan_api::now()).expect("now");
        let changes: Vec<_> = [
            "__cf_bm=new; HttpOnly; Path=/; Domain=claude.ai; Expires=Wed, 21 Oct 2099 18:54:34 GMT",
            "tracker=x; Domain=.example.com",
            "sessionKey=; Max-Age=0; Domain=.claude.ai",
            "routingHint=r1; Path=/",
        ]
        .into_iter()
        .filter_map(|header| cookie_change(header, now))
        .collect();
        Session::update(home.path(), |session| session.apply(&changes)).expect("the update");
        let read = Session::load(home.path())
            .expect("the session loads")
            .expect("there is a session");
        assert_eq!(read.cookie("__cf_bm"), Some("new"));
        assert_eq!(read.cookie("routingHint"), Some("r1"));
        assert_eq!(
            read.cookie("tracker"),
            None,
            "another site's cookie is not kept"
        );
        assert_eq!(
            read.cookie("sessionKey"),
            None,
            "a cleared cookie is dropped"
        );
    }

    #[test]
    fn an_update_after_sign_out_does_not_bring_the_session_back() {
        let home = tempfile::tempdir().expect("a home");
        Session::update(home.path(), |session| session.needs_renewal = true).expect("no-op");
        assert_eq!(Session::state(home.path()), ClaudeAiState::SignedOut);
    }

    #[test]
    fn a_debug_print_never_shows_a_cookie_value() {
        let printed = format!("{:?}", session());
        assert!(printed.contains("sessionKey"));
        assert!(!printed.contains("abc"));
    }
}
