//! Signs in to claude.ai in the user's own browser, and reads the session
//! claude.ai mode runs on.
//!
//! claude.ai's chat API is cookie-authenticated, and Cloudflare in front of
//! it hands out its clearance only to a real browser. So the session comes
//! from one: Chrome (or Edge, Chromium, Brave) is opened on a profile of
//! Artisan's own, the user signs in as they would anywhere, and the cookies
//! and user agent are read over the browser's DevTools protocol.
//!
//! The profile stays, so a later renewal needs no sign-in: when Cloudflare
//! asks for a fresh check, the same profile opens claude.ai off-screen for a
//! few seconds and the new clearance is read the same way.
//!
//! This runs where the user is — `ae` or the Editor — never on a Forge that
//! may be another machine. The session is then handed to the Forge.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, bail};
use artisan_api::ClaudeAiSession;
use futures::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

/// Where a browser signs in.
pub const SIGN_IN_URL: &str = "https://claude.ai/login";
/// What a renewal opens: any signed-in page passes Cloudflare's check.
const RENEW_URL: &str = "https://claude.ai/new";

/// How long the user has to sign in.
const SIGN_IN_TIMEOUT: Duration = Duration::from_mins(10);
/// How long a renewal waits for Cloudflare's new clearance.
const RENEW_TIMEOUT: Duration = Duration::from_secs(20);
/// How long a renewal waits before taking cookies that did not change: a
/// page that loads without a check keeps the clearance it has.
const RENEW_SETTLE: Duration = Duration::from_secs(6);
/// How long the browser has to start and say where DevTools listens.
const START_TIMEOUT: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(750);

/// What the browser is opened for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// A window the user signs in in.
    SignIn,
    /// The profile is already signed in: open claude.ai off-screen and take
    /// the clearance Cloudflare gives it.
    Renew,
}

/// The profile Artisan's claude.ai browser keeps on this device.
///
/// # Errors
///
/// Returns the error when the device has no data folder.
pub fn profile_dir() -> anyhow::Result<PathBuf> {
    Ok(artisan_paths::root()?.join("claude-ai-browser"))
}

/// Whether this device signed in to claude.ai before, so it can renew the
/// session without the user.
#[must_use]
pub fn can_renew() -> bool {
    profile_dir().is_ok_and(|dir| dir.join("Local State").is_file())
}

/// Opens the browser, waits for a signed-in claude.ai session, and returns
/// it. The window closes once the session is read.
///
/// # Errors
///
/// Returns the error when no browser is installed, the user does not sign
/// in in time, or a renewal finds the profile signed out.
pub async fn capture(mode: Mode) -> anyhow::Result<ClaudeAiSession> {
    let profile = profile_dir()?;
    tokio::fs::create_dir_all(&profile)
        .await
        .with_context(|| format!("creating {}", profile.display()))?;
    let url = match mode {
        Mode::SignIn => SIGN_IN_URL,
        Mode::Renew => RENEW_URL,
    };
    let mut devtools = open(&profile, url, mode).await?;
    let result = match mode {
        Mode::SignIn => wait_for_sign_in(&mut devtools).await,
        Mode::Renew => wait_for_renewal(&mut devtools).await,
    };
    // The window is Artisan's; it closes whatever happened.
    devtools.call("Browser.close", json!({})).await.ok();
    result
}

/// The browsers that can sign in, most likely first.
#[must_use]
pub fn browser_candidates() -> Vec<PathBuf> {
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

/// The browser to sign in with, if this device has one.
#[must_use]
pub fn find_browser() -> Option<PathBuf> {
    browser_candidates().into_iter().find(|path| path.is_file())
}

/// A DevTools connection to the browser.
struct DevTools {
    socket: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    next_id: u64,
}

impl DevTools {
    /// Sends one command and waits for its answer, skipping events.
    async fn call(&mut self, method: &str, params: Value) -> anyhow::Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        let request = json!({ "id": id, "method": method, "params": params });
        self.socket
            .send(Message::text(request.to_string()))
            .await
            .with_context(|| format!("sending {method} to the browser"))?;
        let answer = tokio::time::timeout(Duration::from_secs(15), async {
            while let Some(message) = self.socket.next().await {
                let Message::Text(text) = message? else {
                    continue;
                };
                let value: Value = serde_json::from_str(&text)?;
                if value["id"].as_u64() == Some(id) {
                    return Ok(value);
                }
            }
            bail!("the browser closed")
        })
        .await
        .with_context(|| format!("the browser did not answer {method}"))??;
        if let Some(error) = answer.get("error") {
            bail!("the browser refused {method}: {error}");
        }
        Ok(answer["result"].clone())
    }

    /// claude.ai's cookies, as the browser holds them now.
    async fn cookies(&mut self) -> anyhow::Result<Vec<(String, String)>> {
        let result = self.call("Storage.getCookies", json!({})).await?;
        Ok(claude_ai_cookies(&result["cookies"]))
    }

    async fn session(&mut self) -> anyhow::Result<ClaudeAiSession> {
        let cookies = self.cookies().await?;
        let version = self.call("Browser.getVersion", json!({})).await?;
        let user_agent = version["userAgent"]
            .as_str()
            .context("the browser did not say its user agent")?
            .to_owned();
        Ok(ClaudeAiSession {
            cookies,
            user_agent,
        })
    }
}

/// The cookies that belong to claude.ai, out of everything a browser holds.
/// Other sites' cookies never leave the browser.
#[must_use]
pub fn claude_ai_cookies(cookies: &Value) -> Vec<(String, String)> {
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

/// Whether the cookies name the organization claude.ai last used, which a
/// signed-in session always does once the app has loaded.
fn has_org(cookies: &[(String, String)]) -> bool {
    cookies
        .iter()
        .any(|(name, value)| name == "lastActiveOrg" && !value.is_empty())
}

async fn wait_for_sign_in(devtools: &mut DevTools) -> anyhow::Result<ClaudeAiSession> {
    let deadline = tokio::time::Instant::now() + SIGN_IN_TIMEOUT;
    loop {
        let session = devtools.session().await?;
        if session.signed_in() && has_org(&session.cookies) {
            return Ok(session);
        }
        if tokio::time::Instant::now() >= deadline {
            bail!("nobody signed in to claude.ai within ten minutes");
        }
        tokio::time::sleep(POLL).await;
    }
}

async fn wait_for_renewal(devtools: &mut DevTools) -> anyhow::Result<ClaudeAiSession> {
    let before = devtools.cookies().await?;
    let clearance = |cookies: &[(String, String)]| {
        cookies
            .iter()
            .find(|(name, _)| name == "cf_clearance")
            .map(|(_, value)| value.clone())
    };
    let old = clearance(&before);
    let started = tokio::time::Instant::now();
    loop {
        tokio::time::sleep(POLL).await;
        let session = devtools.session().await?;
        let renewed = clearance(&session.cookies).is_some_and(|new| Some(&new) != old.as_ref());
        let settled = started.elapsed() >= RENEW_SETTLE;
        if session.signed_in() && (renewed || settled) {
            return Ok(session);
        }
        if started.elapsed() >= RENEW_TIMEOUT {
            if session.signed_in() {
                return Ok(session);
            }
            bail!("the browser is signed out of claude.ai; sign in again");
        }
    }
}

/// Opens the profile in a browser and connects to it. A window the profile
/// already has open is reused: the browser hands a second launch to it.
async fn open(profile: &Path, url: &str, mode: Mode) -> anyhow::Result<DevTools> {
    let port_file = profile.join("DevToolsActivePort");
    if let Ok(mut devtools) = connect(&port_file).await {
        devtools
            .call("Target.createTarget", json!({ "url": url }))
            .await?;
        return Ok(devtools);
    }
    tokio::fs::remove_file(&port_file).await.ok();
    let browser = find_browser().context(
        "no Chrome, Edge, Chromium or Brave on this device; install one to sign in to claude.ai",
    )?;
    let mut command = tokio::process::Command::new(&browser);
    command
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg("--remote-debugging-port=0")
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if mode == Mode::Renew {
        // Out of sight: the user did not ask for this window.
        command
            .arg("--window-position=-32000,-32000")
            .arg("--window-size=640,480");
    }
    command.arg(url);
    let mut child = command
        .spawn()
        .with_context(|| format!("opening {}", browser.display()))?;
    let deadline = tokio::time::Instant::now() + START_TIMEOUT;
    loop {
        if let Ok(devtools) = connect(&port_file).await {
            // The browser runs on by itself; it is closed over DevTools.
            tokio::spawn(async move { child.wait().await.ok() });
            return Ok(devtools);
        }
        if let Ok(Some(status)) = child.try_wait()
            && !status.success()
        {
            bail!("{} exited ({status})", browser.display());
        }
        if tokio::time::Instant::now() >= deadline {
            child.start_kill().ok();
            bail!("{} did not open DevTools", browser.display());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Connects to the browser the port file names.
async fn connect(port_file: &Path) -> anyhow::Result<DevTools> {
    let text = tokio::fs::read_to_string(port_file).await?;
    let (port, path) = parse_port_file(&text).context("the DevTools port file is incomplete")?;
    let url = format!("ws://127.0.0.1:{port}{path}");
    let (socket, _) = tokio::time::timeout(
        Duration::from_secs(5),
        tokio_tungstenite::connect_async(url.as_str()),
    )
    .await
    .context("the browser did not take the DevTools connection")??;
    Ok(DevTools { socket, next_id: 0 })
}

/// The port and browser path a `DevToolsActivePort` file names.
fn parse_port_file(text: &str) -> Option<(u16, String)> {
    let mut lines = text.lines();
    let port = lines.next()?.trim().parse().ok()?;
    let path = lines.next()?.trim();
    path.starts_with("/devtools/browser/")
        .then(|| (port, path.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_claude_ai_cookies_leave_the_browser() {
        let cookies = json!([
            { "name": "sessionKey", "value": "abc", "domain": ".claude.ai" },
            { "name": "lastActiveOrg", "value": "org", "domain": "claude.ai" },
            { "name": "SID", "value": "google", "domain": ".google.com" },
            { "name": "evil", "value": "x", "domain": "notclaude.ai" },
        ]);
        assert_eq!(
            claude_ai_cookies(&cookies),
            vec![
                ("sessionKey".to_owned(), "abc".to_owned()),
                ("lastActiveOrg".to_owned(), "org".to_owned()),
            ]
        );
    }

    #[test]
    fn the_port_file_names_the_browser_endpoint() {
        assert_eq!(
            parse_port_file("9222\n/devtools/browser/abc-123\n"),
            Some((9222, "/devtools/browser/abc-123".to_owned()))
        );
        assert_eq!(parse_port_file("9222\n"), None);
        assert_eq!(parse_port_file("x\n/devtools/browser/a"), None);
    }
}
