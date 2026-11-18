mod authorization;
mod devtools;
mod renewal;
mod session;

use std::path::{Path, PathBuf};
use std::time::Duration;

use artisan_api::ClaudeAiSession;
use serde_json::Value;

pub use authorization::AuthorizationError;
pub use devtools::DevToolsError;
pub use renewal::{Renewal, Step, Ticket};
pub use session::OpenError;

use authorization::Authorizations;
use devtools::DevTools;
use session::BrowserSession;

const SIGN_IN_URL: &str = "https://claude.ai/login";
const RENEW_URL: &str = "https://claude.ai/new";

const SIGN_IN_TIMEOUT: Duration = Duration::from_mins(10);
const RENEW_TIMEOUT: Duration = Duration::from_secs(20);
const RENEW_SETTLE: Duration = Duration::from_secs(6);
const POLL: Duration = Duration::from_millis(750);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    SignIn,
    Renew,
}

#[derive(Debug, thiserror::Error)]
pub enum SignInError {
    #[error("this device has no data folder for the claude.ai browser profile")]
    NoDataFolder(#[source] artisan_paths::RootError),
    #[error("opening a browser to sign in to claude.ai")]
    Open(#[source] OpenError),
    #[error("reading the claude.ai session from the browser")]
    Read(#[source] DevToolsError),
    #[error("nobody signed in to claude.ai within ten minutes")]
    TimedOut,
}

#[derive(Debug, thiserror::Error)]
pub enum RenewError {
    #[error("this device has no data folder for the claude.ai browser profile")]
    NoDataFolder(#[source] artisan_paths::RootError),
    #[error("checking that this device signed {host} in to claude.ai")]
    Authorizations {
        host: String,
        #[source]
        source: AuthorizationError,
    },
    #[error("this device did not sign {host} in to claude.ai, so it does not renew its session")]
    NotAuthorized { host: String },
    #[error("opening the browser to renew the claude.ai session for {host}")]
    Open {
        host: String,
        #[source]
        source: OpenError,
    },
    #[error("reading the renewed claude.ai session for {host} from the browser")]
    Read {
        host: String,
        #[source]
        source: DevToolsError,
    },
    #[error("the browser is signed out of claude.ai; sign in again")]
    SignedOut,
    #[error(
        "the browser is signed in to a different claude.ai account than the one {host} was signed in with; sign {host} in again"
    )]
    AccountChanged { host: String },
}

pub fn profile_dir() -> Result<PathBuf, artisan_paths::RootError> {
    Ok(artisan_paths::root()?.join("claude-ai-browser"))
}

#[must_use]
pub fn has_profile() -> bool {
    profile_dir().is_ok_and(|dir| dir.join("Local State").is_file())
}

#[must_use]
pub fn can_renew(host: &str) -> bool {
    has_profile()
        && profile_dir().is_ok_and(|dir| {
            Authorizations::read(&dir)
                .is_ok_and(|authorizations| authorizations.organization(host).is_some())
        })
}

pub fn authorize(host: &str, session: &ClaudeAiSession) -> Result<(), AuthorizationError> {
    let profile = profile_dir().map_err(AuthorizationError::NoDataFolder)?;
    authorization::authorize_in(&profile, host, session)
}

pub async fn sign_in() -> Result<ClaudeAiSession, SignInError> {
    let profile = profile_dir().map_err(SignInError::NoDataFolder)?;
    sign_in_with(&profile, find_browser).await
}

pub async fn renew(host: &str) -> Result<ClaudeAiSession, RenewError> {
    let profile = profile_dir().map_err(RenewError::NoDataFolder)?;
    renew_with(&profile, host, find_browser).await
}

async fn sign_in_with(
    profile: &Path,
    browser: impl FnOnce() -> Option<PathBuf>,
) -> Result<ClaudeAiSession, SignInError> {
    let mut session = BrowserSession::open(profile, SIGN_IN_URL, Mode::SignIn, browser)
        .await
        .map_err(SignInError::Open)?;
    let result = wait_for_sign_in(session.devtools()).await;
    session.close().await;
    result
}

async fn renew_with(
    profile: &Path,
    host: &str,
    browser: impl FnOnce() -> Option<PathBuf>,
) -> Result<ClaudeAiSession, RenewError> {
    let authorized = |profile: &Path| {
        Authorizations::read(profile)
            .map_err(|source| RenewError::Authorizations {
                host: host.to_owned(),
                source,
            })?
            .organization(host)
            .map(str::to_owned)
            .ok_or_else(|| RenewError::NotAuthorized {
                host: host.to_owned(),
            })
    };
    let organization = authorized(profile)?;
    let mut session = BrowserSession::open(profile, RENEW_URL, Mode::Renew, browser)
        .await
        .map_err(|source| RenewError::Open {
            host: host.to_owned(),
            source,
        })?;
    let result = wait_for_renewal(session.devtools()).await;
    session.close().await;
    let renewed = result.map_err(|error| match error {
        Renewed::Read(source) => RenewError::Read {
            host: host.to_owned(),
            source,
        },
        Renewed::SignedOut => RenewError::SignedOut,
    })?;
    if authorized(profile)? != organization || !authorization::same_account(&organization, &renewed)
    {
        return Err(RenewError::AccountChanged {
            host: host.to_owned(),
        });
    }
    Ok(renewed)
}

#[must_use]
fn browser_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if cfg!(windows) {
        let roots = ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"]
            .into_iter()
            .filter_map(std::env::var_os)
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        for relative in [
            r"Google\Chrome\Application\chrome.exe",
            r"Microsoft\Edge\Application\msedge.exe",
            r"BraveSoftware\Brave-Browser\Application\brave.exe",
            r"Chromium\Application\chrome.exe",
        ] {
            candidates.extend(roots.iter().map(|root| root.join(relative)));
        }
    } else if cfg!(target_os = "macos") {
        for app in [
            "Google Chrome.app/Contents/MacOS/Google Chrome",
            "Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            "Brave Browser.app/Contents/MacOS/Brave Browser",
            "Chromium.app/Contents/MacOS/Chromium",
        ] {
            candidates.push(Path::new("/Applications").join(app));
            if let Ok(home) = artisan_paths::home() {
                candidates.push(home.join("Applications").join(app));
            }
        }
    } else {
        let path = std::env::var_os("PATH").unwrap_or_default();
        for name in [
            "google-chrome",
            "google-chrome-stable",
            "chromium",
            "chromium-browser",
            "microsoft-edge",
            "microsoft-edge-stable",
            "brave-browser",
        ] {
            candidates.extend(std::env::split_paths(&path).map(|dir| dir.join(name)));
        }
    }
    candidates
}

#[must_use]
pub fn find_browser() -> Option<PathBuf> {
    browser_candidates().into_iter().find(|path| path.is_file())
}

#[must_use]
fn claude_ai_cookies(cookies: &Value) -> Vec<(String, String)> {
    cookies
        .as_array()
        .into_iter()
        .flatten()
        .filter(|cookie| {
            cookie["domain"].as_str().is_some_and(|domain| {
                let domain = domain.trim_start_matches('.');
                domain == "claude.ai" || domain.ends_with(".claude.ai")
            })
        })
        .filter_map(|cookie| {
            Some((
                cookie["name"].as_str()?.to_owned(),
                cookie["value"].as_str()?.to_owned(),
            ))
        })
        .collect()
}

fn has_org(cookies: &[(String, String)]) -> bool {
    cookies
        .iter()
        .any(|(name, value)| name == authorization::ORGANIZATION_COOKIE && !value.is_empty())
}

async fn wait_for_sign_in(devtools: &mut DevTools) -> Result<ClaudeAiSession, SignInError> {
    let deadline = tokio::time::Instant::now() + SIGN_IN_TIMEOUT;
    loop {
        let session = devtools.session().await.map_err(SignInError::Read)?;
        if session.signed_in() && has_org(&session.cookies) {
            return Ok(session);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(SignInError::TimedOut);
        }
        tokio::time::sleep(POLL).await;
    }
}

enum Renewed {
    Read(DevToolsError),
    SignedOut,
}

async fn wait_for_renewal(devtools: &mut DevTools) -> Result<ClaudeAiSession, Renewed> {
    let clearance = |cookies: &[(String, String)]| {
        cookies
            .iter()
            .find(|(name, _)| name == "cf_clearance")
            .map(|(_, value)| value.clone())
    };
    let old = clearance(&devtools.cookies().await.map_err(Renewed::Read)?);
    let started = tokio::time::Instant::now();
    loop {
        tokio::time::sleep(POLL).await;
        let session = devtools.session().await.map_err(Renewed::Read)?;
        let renewed = clearance(&session.cookies).is_some_and(|new| Some(&new) != old.as_ref());
        let settled = started.elapsed() >= RENEW_SETTLE;
        if session.signed_in() && (renewed || settled) {
            return Ok(session);
        }
        if started.elapsed() >= RENEW_TIMEOUT {
            if session.signed_in() {
                return Ok(session);
            }
            return Err(Renewed::SignedOut);
        }
    }
}

#[cfg(test)]
#[path = "../tests/lib.rs"]
mod tests;
