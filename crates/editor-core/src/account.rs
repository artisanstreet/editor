//! AST account sign-in, sessions, and machine inventory for the native Editor.
//!
//! The account authority is Core (opaque HTTP). This controller owns the
//! explicit sign-in state machine, persists only the opaque session token,
//! and exposes the machine inventory plus the Relay handoff. There are no
//! passwords, passkeys, or social logins: magic email links only, with an
//! optional TOTP second factor that email alone never bypasses.
//!
//! Transport and wire shapes live in `artisan_account::network` (shared,
//! reusable); this file holds controller state, persistence, the `ae`
//! subprocess pairing, and the settings UI wiring. No business logic auth
//! stores or cryptography beyond what `artisan-account` owns. Responses are
//! read forward-compatibly; requests never carry unknown fields.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result};
use artisan_account::network::{AccountProfile, CoreSessionClient, NetworkError, SessionToken};
use artisan_account::normalise_code;
use gpui::{Context, Entity, Task};
use serde::{Deserialize, Serialize};

use super::device_name;
use super::{ForgeClient, HostConfig};
use crate::Core;
use crate::hosts::EditorFiles;

/// Where Core lives when neither the account file nor the environment says
/// otherwise (production; `AST_CORE_BASE_URL` overrides for a dev Core).
pub const CORE_BASE_URL_DEFAULT: &str = "https://api.artisan.st";

/// Base URL resolution: account file, else `AST_CORE_BASE_URL`, else production.
fn core_base_url(saved: Option<&str>) -> String {
    saved
        .map(str::to_owned)
        .or_else(|| std::env::var("AST_CORE_BASE_URL").ok())
        .filter(|url| !url.trim().is_empty())
        .unwrap_or_else(|| CORE_BASE_URL_DEFAULT.to_owned())
}

/// User-facing text for Core failures. Never echoes server text (it may
/// name internals); the request id is in the log, not on screen.
fn plain_failure(error: &NetworkError) -> String {
    match error {
        NetworkError::Core { code, .. } => match code.as_str() {
            "bad_request" => "Core could not use that request.".to_owned(),
            "unauthorized" => "That sign-in link is invalid or already used.".to_owned(),
            "forbidden" => {
                "The email link is not approved yet, or the second factor is missing.".to_owned()
            }
            "gone" => "That sign-in request expired. Request a new link.".to_owned(),
            "rate_limited" => "Too many attempts. Wait a while and try again.".to_owned(),
            _ => "Core could not do that right now. Try again in a bit.".to_owned(),
        },
        NetworkError::Transport => {
            "Could not reach Core. Check the connection and try again.".to_owned()
        }
    }
}

// ---------------------------------------------------------------------------
// Session file: `{editor-data}/account.json`, 0600, atomic replace.
// Only the base URL and the opaque session token are stored; the profile and
// machines are re-fetched, so a revoke takes effect on next launch.
// ---------------------------------------------------------------------------

fn account_path(files: &EditorFiles) -> std::path::PathBuf {
    files.dir.join("account.json")
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct AccountFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session_token: Option<String>,
}

fn load_account_file(path: &std::path::Path) -> Result<AccountFile> {
    match std::fs::read(path) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).with_context(|| format!("read {}", path.display()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(AccountFile::default()),
        Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
    }
}

fn save_account_file(path: &std::path::Path, account: &AccountFile) -> Result<()> {
    use std::io::Write as _;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let parent = path.parent().context("configuration directory")?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(parent).context("create configuration temporary")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o600))?;
    }
    serde_json::to_writer_pretty(temporary.as_file_mut(), account)?;
    temporary.flush()?;
    temporary
        .persist(path)
        .with_context(|| format!("replace {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Relay handoff: the controller prepares everything up to the channel —
// session-authed grant plus the target Forge's enrolled key — and hands a
// `RelayDialRequest` to Vilde's transport, which owns `ForgeClient`
// construction (`via_relay`), grant refresh, and the channel. The dialer
// returns a connected Forge client, which the controller puts into the
// existing host inventory without persisting relay-only entries.
// ---------------------------------------------------------------------------

/// Everything the Relay dialer needs to open a channel to one Forge.
/// `Debug` redacts the grant and the `cnf` secret.
#[derive(Clone)]
pub struct RelayDialRequest {
    pub account_id: String,
    pub machine_id: String,
    pub machine_label: String,
    /// The Forge's enrolled Ed25519 key (base64url), to verify its
    /// handshake transcript against — from the authenticated machine list.
    pub forge_ed25519_b64u: String,
    /// Short-lived Core grant JWT (`aud=ast-relay`), presented in-band.
    pub grant_jwt: String,
    /// Ephemeral `cnf` private key for transcript binding. Zeroed on drop.
    pub cnf_secret: [u8; 32],
}

impl std::fmt::Debug for RelayDialRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RelayDialRequest")
            .field("account_id", &self.account_id)
            .field("machine_id", &self.machine_id)
            .field("machine_label", &self.machine_label)
            .field("forge_ed25519_b64u", &self.forge_ed25519_b64u)
            .field("grant_jwt", &"(redacted)")
            .field("cnf_secret", &"(redacted)")
            .finish()
    }
}

impl Drop for RelayDialRequest {
    fn drop(&mut self) {
        self.cnf_secret.fill(0);
    }
}

/// Opens a Relay channel for a prepared request.
///
/// Returns a connected Forge client, which the controller puts into the
/// existing (ephemeral, never persisted) host inventory via
/// `Core::connect_ephemeral`. Provided by Vilde's transport
/// (`ForgeClient::via_relay` or equivalent); it owns grant refresh from here on.
pub type RelayDialer =
    Arc<dyn Fn(RelayDialRequest) -> Task<Result<(HostConfig, ForgeClient)>> + Send + Sync>;

/// Builds a session-bound dialer for the current Relay URL. Registered once
/// by Vilde's transport at startup; the controller calls it on every login
/// and resumed session with the live session token.
pub type RelayDialerFactory =
    Arc<dyn Fn(SessionToken, String) -> Option<RelayDialer> + Send + Sync>;

/// First-party production Relay; `AST_RELAY_URL` overrides for dev.
/// Explicit empty `AST_RELAY_URL` disables the Relay path.
pub const RELAY_BASE_URL_DEFAULT: &str = "wss://relay.editor.artisan.st";

/// Relay URL: explicit `AST_RELAY_URL` wins, else the production default.
/// An explicitly empty `AST_RELAY_URL` disables Relay (returns `None`);
/// an unset variable returns the production default so stock installs can
/// connect account Forges without extra configuration.
pub fn relay_url_from_env() -> Option<String> {
    std::env::var("AST_RELAY_URL")
        .ok()
        .and_then(|url| {
            let trimmed = url.trim().to_owned();
            (!trimmed.is_empty()).then_some(trimmed)
        })
        .or_else(|| Some(RELAY_BASE_URL_DEFAULT.to_owned()))
}

// ---------------------------------------------------------------------------
// Controller: the explicit sign-in state machine.
// ---------------------------------------------------------------------------

/// Every state the Account page can be in. No hidden booleans.
///
/// `Debug` is manual: request secrets never print.
#[derive(Clone, PartialEq, Eq)]
pub enum AccountPhase {
    /// No session. The email form is the whole story.
    SignedOut,
    /// Saved session is validating against `/v1/me`; unverified.
    /// Distinct from `Requesting` so startup never flashes signed-out and
    /// pending refresh never drops a valid session on transport failure.
    Restoring,
    /// `POST /v1/auth/magic/request` in flight.
    Requesting,
    /// Link sent; polling `/v1/auth/magic/status` until the email is
    /// approved, the request completes, or it expires.
    Pending {
        request_id: String,
        request_secret: String,
        email: String,
        expires_at: String,
    },
    /// The email side approved. Polling stopped; the user explicitly
    /// finishes here (the original-session approval for cross-browser flows).
    EmailApproved {
        request_id: String,
        request_secret: String,
        email: String,
        totp_required: bool,
    },
    /// `POST /v1/auth/magic/finalize` in flight.
    Finalizing,
    /// Signed in. Profile and machines come from `/v1/me`.
    Authenticated { profile: AccountProfile },
    /// Plain-language failure. Dismissing returns to `SignedOut`.
    Error { message: String },
}

impl std::fmt::Debug for AccountPhase {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SignedOut => formatter.write_str("SignedOut"),
            Self::Restoring => formatter.write_str("Restoring"),
            Self::Requesting => formatter.write_str("Requesting"),
            Self::Pending {
                request_id,
                email,
                expires_at,
                ..
            } => formatter
                .debug_struct("Pending")
                .field("request_id", request_id)
                .field("request_secret", &"(redacted)")
                .field("email", email)
                .field("expires_at", expires_at)
                .finish(),
            Self::EmailApproved {
                request_id,
                email,
                totp_required,
                ..
            } => formatter
                .debug_struct("EmailApproved")
                .field("request_id", request_id)
                .field("request_secret", &"(redacted)")
                .field("email", email)
                .field("totp_required", totp_required)
                .finish(),
            Self::Finalizing => formatter.write_str("Finalizing"),
            Self::Authenticated { profile } => formatter
                .debug_struct("Authenticated")
                .field("profile", profile)
                .finish(),
            Self::Error { message } => formatter.debug_tuple("Error").field(message).finish(),
        }
    }
}

impl AccountPhase {
    #[must_use]
    pub const fn signed_in(&self) -> bool {
        matches!(self, Self::Authenticated { .. })
    }
}

pub enum AccountEvent {
    Changed,
}

impl gpui::EventEmitter<AccountEvent> for Account {}

/// What a wrong pairing code looks like before it ever reaches Core.
#[must_use]
pub fn valid_code_shape(code: &str) -> bool {
    let normalised = normalise_code(code);
    !normalised.is_empty() && normalised.len() <= 16
}

/// One enrolled Forge as `/v1/me` reports it. Reuses the existing network
/// DTO directly; no new shape.
pub type AccountMachine = artisan_account::network::MachineEntry;

mod enrollment;

pub struct Account {
    files: EditorFiles,
    http: CoreSessionClient,
    base_url: String,
    phase: AccountPhase,
    session: Option<SessionToken>,
    /// The running status poll; replacing it cancels the old one.
    poll: Option<Task<()>>,
    /// Transient confirmation ("Code claimed for you@…"), cleared on state change.
    notice: Option<String>,
    relay_error: Option<String>,
    /// Last `ae account …` output shown on the page (read-only info; the
    /// actual approval still happens explicitly on the Forge).
    forge_status_output: Option<String>,
    /// Session-bound Relay dialer, installed on login/resume once Vilde's
    /// transport registers its factory on `Core`.
    dialer: Option<RelayDialer>,
    dialer_factory: Option<RelayDialerFactory>,
    /// Relay URL from `AST_RELAY_URL` (Vilde's protocol docs own the value;
    /// never invented here). Needed alongside the session to build the dialer.
    relay_url: Option<String>,
    /// Bumped on every session set/clear; stale async callbacks compare it.
    generation: u64,
    /// Sign-in request epoch. Bumped on every new magic-link request and on
    /// cancel/sign-out, so a detached request HTTP callback can never set
    /// `Pending` (or `Error`) over a newer flow or a locked gate. Session
    /// refresh never bumps this: ordinary refresh must not rotate anything.
    auth_epoch: u64,
    /// Cached local Forge binding. Refreshed only via explicit
    /// `refresh_local_binding` (user action / onboarding step), never per
    /// render, so the hot path does no disk IO.
    cached_binding: Option<crate::access::LocalForgeBinding>,
}

impl Account {
    pub fn new(files: EditorFiles, cx: &mut Context<Self>) -> Self {
        Self::with_base_url(files, None, cx)
    }

    #[expect(
        clippy::expect_used,
        reason = "reqwest client with only timeouts builds unless TLS is broken; Account holds a client, not a Result"
    )]
    pub fn with_base_url(
        files: EditorFiles,
        base_url: Option<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let stored = load_account_file(&account_path(&files)).unwrap_or_default();
        let base_url = base_url
            .or_else(|| stored.base_url.clone())
            .unwrap_or_else(|| core_base_url(None));
        let http = CoreSessionClient::new(&base_url).expect("Core HTTP client builds");
        // Install Vilde's Relay dialer factory up front so login/resume can
        // build a session-bound dialer without further wiring. The factory
        // itself is his transport code; the controller only calls it.
        let dialer_factory = Some(crate::client::relay_dialer_factory(
            http.clone(),
            cx.background_executor().clone(),
        ));
        let mut this = Self {
            files,
            http,
            base_url,
            phase: AccountPhase::SignedOut,
            session: stored.session_token.map(SessionToken::new),
            poll: None,
            notice: None,
            relay_error: None,
            forge_status_output: None,
            dialer: None,
            dialer_factory,
            relay_url: relay_url_from_env(),
            generation: 0,
            auth_epoch: 0,
            cached_binding: None,
        };
        if this.session.is_some() {
            this.resume(cx);
        }
        this
    }

    /// Registers the Relay dialer factory (Vilde's transport, once at
    /// startup). The controller installs a session-bound dialer on every
    /// login and resumed session.
    pub fn set_relay_dialer_factory(&mut self, factory: RelayDialerFactory) {
        self.dialer_factory = Some(factory);
        self.install_relay_dialer();
    }

    /// Session-authed Core client backing the Relay grant refresh.
    /// `Core::new` clones this to build Vilde's dialer factory via the
    /// exact startup call he requests.
    #[must_use]
    pub fn session_client(&self) -> CoreSessionClient {
        self.http.clone()
    }

    /// The live session token, if any. Core tracks it to close account
    /// channels when the session is replaced even under the same account.
    #[must_use]
    pub fn session_token(&self) -> Option<SessionToken> {
        self.session.clone()
    }

    /// Core API origin this controller talks to. Onboarding records are
    /// scoped by it so dev/prod account ids never collide.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Monotonic session generation: bumped on every session set/clear so
    /// stale in-flight callbacks cannot repopulate after sign-out/switch.
    /// Types land first; the bump wiring follows with the lifecycle work.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Builds the session-bound dialer when a session, factory, and relay
    /// URL are all present. Called after finalize and after resume validation.
    fn install_relay_dialer(&mut self) {
        let installed = match (&self.session, &self.dialer_factory, &self.relay_url) {
            (Some(session), Some(factory), Some(url)) => factory(session.clone(), url.clone()),
            _ => None,
        };
        self.dialer = installed;
        if self.dialer.is_some() {
            self.relay_error = None;
        }
    }

    #[must_use]
    pub const fn phase(&self) -> &AccountPhase {
        &self.phase
    }

    #[must_use]
    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    #[must_use]
    pub fn relay_error(&self) -> Option<&str> {
        self.relay_error.as_deref()
    }

    #[must_use]
    pub fn forge_status_output(&self) -> Option<&str> {
        self.forge_status_output.as_deref()
    }

    fn set_phase(&mut self, phase: AccountPhase, cx: &mut Context<Self>) {
        self.poll = None;
        if !matches!(phase, AccountPhase::Error { .. }) {
            self.notice = None;
        }
        self.phase = phase;
        cx.emit(AccountEvent::Changed);
        cx.notify();
    }

    fn fail(&mut self, error: &NetworkError, cx: &mut Context<Self>) {
        tracing::warn!(code = ?error, "account flow failed");
        self.set_phase(
            AccountPhase::Error {
                message: plain_failure(error),
            },
            cx,
        );
    }

    /// Invalidates all in-flight callbacks from the previous session.
    const fn bump_generation(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    /// Operational failure while signed in: preserves the valid session and
    /// profile, surfacing friendly text as a notice. Only an explicitly
    /// dead session (`unauthorized`/`gone`) locks. Login request/finalize
    /// errors keep using entry `Error`.
    fn fail_operational(&mut self, error: &NetworkError, cx: &mut Context<Self>) {
        if crate::access::is_session_dead(error) {
            tracing::warn!(code = ?error, "session is dead; locking");
            self.clear_session_locked(cx);
            return;
        }
        tracing::warn!(code = ?error, "account operation failed; session kept");
        self.notice = Some(plain_failure(error));
        cx.emit(AccountEvent::Changed);
        cx.notify();
    }

    /// Clears the session after Core rejects it. Bumps the generation and
    /// the auth epoch so stale `finalize`/`refresh`/`connect_machine` and
    /// detached request callbacks cannot repopulate inventory or unlock
    /// after sign-out/switch.
    fn clear_session_locked(&mut self, cx: &mut Context<Self>) {
        self.session = None;
        self.dialer = None;
        self.relay_error = None;
        self.clear_session_file();
        self.bump_generation();
        self.auth_epoch = self.auth_epoch.wrapping_add(1);
        self.set_phase(AccountPhase::SignedOut, cx);
    }

    /// The signed-in profile, if any.
    #[must_use]
    pub const fn profile(&self) -> Option<&AccountProfile> {
        match &self.phase {
            AccountPhase::Authenticated { profile } => Some(profile),
            AccountPhase::SignedOut
            | AccountPhase::Restoring
            | AccountPhase::Requesting
            | AccountPhase::Pending { .. }
            | AccountPhase::EmailApproved { .. }
            | AccountPhase::Finalizing
            | AccountPhase::Error { .. } => None,
        }
    }

    /// Non-revoked Forges from the live profile. Empty unless signed in.
    #[must_use]
    pub fn onboardable_machines(&self) -> Vec<AccountMachine> {
        self.profile()
            .map(|profile| {
                profile
                    .machines
                    .iter()
                    .filter(|machine| machine.revoked_at.is_none())
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Whether `machine_id` is in the live profile and non-revoked.
    #[must_use]
    pub fn can_complete_setup(&self, machine_id: &str) -> bool {
        self.profile().is_some_and(|profile| {
            profile
                .machines
                .iter()
                .any(|machine| machine.id == machine_id && machine.revoked_at.is_none())
        })
    }


    // -- sign-in ------------------------------------------------------

    /// Starts a magic-link request. The secret stays in memory; only the
    /// emailed token leaves the device, and it alone can never sign in.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "callers hand over an owned email string; the public controller API keeps String"
    )]
    pub fn request_link(&mut self, email: String, cx: &mut Context<Self>) {
        let email = email.trim().to_owned();
        if !valid_email(&email) {
            self.set_phase(
                AccountPhase::Error {
                    message: "Enter the email address for the account.".to_owned(),
                },
                cx,
            );
            return;
        }
        let http = self.http.clone();
        let device_name = device_name();
        let email_owned = email.clone();
        // A new request retires any detached earlier request: only this
        // epoch may move the gate to `Pending`.
        self.auth_epoch = self.auth_epoch.wrapping_add(1);
        let auth_epoch = self.auth_epoch;
        self.set_phase(AccountPhase::Requesting, cx);
        cx.spawn(async move |this, cx| {
            let result =
                run_http(async move { http.magic_request(&email, &device_name).await }).await;
            let email = email_owned;
            this.update(cx, |this, cx| {
                if this.auth_epoch != auth_epoch {
                    return;
                }
                match result {
                Ok(requested) => {
                    let phase = AccountPhase::Pending {
                        request_id: requested.request_id,
                        request_secret: requested.request_secret,
                        email,
                        expires_at: requested.expires_at,
                    };
                    this.set_phase(phase, cx);
                    this.poll_status(cx);
                }
                Err(error) => this.fail(&error, cx),
                }
            })
            .ok();
        })
        .detach();
    }

    /// Polls the request until approval, completion, or expiry. Replacing
    /// the poll task cancels the previous loop: exactly one runs at a time.
    fn poll_status(&mut self, cx: &Context<Self>) {
        let (request_id, secret) = match &self.phase {
            AccountPhase::Pending {
                request_id,
                request_secret,
                ..
            } => (request_id.clone(), request_secret.clone()),
            AccountPhase::SignedOut
            | AccountPhase::Restoring
            | AccountPhase::Requesting
            | AccountPhase::EmailApproved { .. }
            | AccountPhase::Finalizing
            | AccountPhase::Authenticated { .. }
            | AccountPhase::Error { .. } => return,
        };
        let http = self.http.clone();
        self.poll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(3)).await;
                let status = run_http({
                    let http = http.clone();
                    let request_id = request_id.clone();
                    let secret = secret.clone();
                    async move { http.magic_status(&request_id, &secret).await }
                })
                .await;
                let done = this
                    .update(cx, |this, cx| match status {
                        Err(error) => {
                            this.fail(&error, cx);
                            true
                        }
                        Ok(status) => {
                            if status.status == "email_approved" {
                                this.set_phase(
                                    AccountPhase::EmailApproved {
                                        request_id: request_id.clone(),
                                        request_secret: secret.clone(),
                                        email: status.email,
                                        totp_required: status.totp_required,
                                    },
                                    cx,
                                );
                                return true;
                            }
                            if status.status != "pending" {
                                this.set_phase(
                                    AccountPhase::Error {
                                        message:
                                            "That sign-in request expired. Request a new link."
                                                .to_owned(),
                                    },
                                    cx,
                                );
                                return true;
                            }
                            false
                        }
                    })
                    .unwrap_or(true);
                if done {
                    return;
                }
            }
        }));
    }

    /// Explicit finish after the email side approved (the original-session
    /// approval for cross-browser flows). `totp_code` or `recovery_code`
    /// when the account has TOTP; either may be left empty otherwise.
    pub fn finish_sign_in(
        &mut self,
        totp_code: Option<String>,
        recovery_code: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let (request_id, secret) = match &self.phase {
            AccountPhase::EmailApproved {
                request_id,
                request_secret,
                ..
            } => (request_id.clone(), request_secret.clone()),
            AccountPhase::SignedOut
            | AccountPhase::Restoring
            | AccountPhase::Requesting
            | AccountPhase::Pending { .. }
            | AccountPhase::Finalizing
            | AccountPhase::Authenticated { .. }
            | AccountPhase::Error { .. } => return,
        };
        let http = self.http.clone();
        let generation = self.generation;
        self.set_phase(AccountPhase::Finalizing, cx);
        cx.spawn(async move |this, cx| {
            let result = run_http(async move {
                http.magic_finalize(
                    &request_id,
                    &secret,
                    totp_code.as_deref().filter(|code| !code.trim().is_empty()),
                    recovery_code
                        .as_deref()
                        .filter(|code| !code.trim().is_empty()),
                )
                .await
            })
            .await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                match result {
                Err(error) => this.fail(&error, cx),
                Ok((finalized, token)) => {
                    this.session = Some(token);
                    this.bump_generation();
                    let generation = this.generation;
                    if let Err(error) = this.persist_session() {
                        tracing::warn!("could not persist the session: {error:#}");
                    }
                    let session = this.session.clone();
                    let http = this.http.clone();
                    cx.spawn(async move |this, cx| {
                        let profile = match session {
                            Some(session) => run_http(async move { http.me(&session).await }).await,
                            None => Err(NetworkError::Transport),
                        };
                        this.update(cx, |this, cx| {
                            if this.generation != generation {
                                return;
                            }
                            match profile {
                            Ok(profile) => {
                                this.install_relay_dialer();
                                this.set_phase(AccountPhase::Authenticated { profile }, cx);
                            }
                            Err(error) => this.fail(&error, cx),
                        }
                        })
                        .ok();
                    })
                    .detach();
                    let _ = finalized;
                }
            }
            })
            .ok();
        })
        .detach();
    }

    fn persist_session(&self) -> Result<()> {
        save_account_file(
            &account_path(&self.files),
            &AccountFile {
                base_url: Some(self.base_url.clone()),
                session_token: self.session.as_ref().map(|token| token.as_str().to_owned()),
            },
        )
    }

    fn clear_session_file(&self) {
        if let Err(error) = save_account_file(
            &account_path(&self.files),
            &AccountFile {
                // Keep the explicit Core origin across sign-out; only the
                // opaque token clears so a saved custom Core survives.
                base_url: Some(self.base_url.clone()),
                session_token: None,
            },
        ) {
            tracing::warn!("could not clear the saved session: {error:#}");
        }
    }

    /// Validates a saved session on startup and loads the inventory.
    /// A dead session falls back to signed-out without blocking the UI.
    /// Transport failures keep the saved session and stay `Restoring`.
    fn resume(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let http = self.http.clone();
        let generation = self.generation;
        self.set_phase(AccountPhase::Restoring, cx);
        cx.spawn(async move |this, cx| {
            let profile = run_http(async move { http.me(&session).await }).await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                match profile {
                    Ok(profile) => {
                        this.install_relay_dialer();
                        this.set_phase(AccountPhase::Authenticated { profile }, cx);
                    }
                    Err(error) if crate::access::is_session_invalid(&error) => {
                        this.clear_session_locked(cx);
                    }
                    Err(_) => {
                        // Offline or transient Core failure: keep the saved
                        // session and stay Restoring, never flash signed-out.
                        if matches!(this.phase(), AccountPhase::Restoring) {
                            cx.emit(AccountEvent::Changed);
                            cx.notify();
                        } else {
                            this.set_phase(AccountPhase::Restoring, cx);
                        }
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    /// Re-reads `/v1/me`: profile plus machine inventory. Transport or
    /// non-auth failures keep the existing `Authenticated` profile; only a
    /// session-invalidating Core code signs out.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.session.clone() else {
            self.set_phase(AccountPhase::SignedOut, cx);
            return;
        };
        let http = self.http.clone();
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let profile = run_http(async move { http.me(&session).await }).await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                match profile {
                Ok(profile) => {
                    this.install_relay_dialer();
                    this.set_phase(AccountPhase::Authenticated { profile }, cx);
                }
                Err(error) if crate::access::is_session_invalid(&error) => {
                    this.clear_session_locked(cx);
                }
                Err(_) => {
                    // Keep the valid session; a failed refresh must not drop
                    // it. Surface nothing as an auth error.
                    cx.notify();
                }
            }
            })
            .ok();
        })
        .detach();
    }

    /// Signs out of this device: revokes the session, clears the file.
    pub fn sign_out(&mut self, cx: &mut Context<Self>) {
        if let Some(session) = self.session.clone() {
            let http = self.http.clone();
            crate::runtime::runtime().spawn(async move {
                if let Err(error) = http.revoke_current(&session).await {
                    tracing::warn!("sign-out revoke failed: {error:#}");
                }
            });
        }
        self.clear_session_locked(cx);
    }

    /// Signs out every session on the account, then this device.
    pub fn sign_out_everywhere(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.session.clone() else {
            self.clear_session_locked(cx);
            return;
        };
        let http = self.http.clone();
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let result = run_http(async move { http.revoke_all_sessions(&session).await }).await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                if let Err(error) = result {
                    this.fail_operational(&error, cx);
                } else {
                    this.clear_session_locked(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Claims an enrollment code from `ae account enroll` into this account.
    /// Shows the exact account it lands in; the Forge still confirms locally.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "callers hand over an owned code string; the public controller API keeps String"
    )]
    pub fn claim_code(&mut self, code: String, cx: &mut Context<Self>) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let code = normalise_code(&code);
        if !valid_code_shape(&code) {
            self.notice = Some(
                "That code does not look complete. Check the Forge output and try again."
                    .to_owned(),
            );
            cx.emit(AccountEvent::Changed);
            cx.notify();
            return;
        }
        let http = self.http.clone();
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let result =
                run_http(async move { http.claim_enrollment(&session, &code).await }).await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                match result {
                    Ok(_) => {
                        let email = this
                            .profile()
                            .map(|profile| profile.email.clone())
                            .unwrap_or_default();
                        this.notice = Some(if email.is_empty() {
                            "Code claimed. Confirm it on the Forge.".to_owned()
                        } else {
                            format!(
                                "Code claimed for {email}. On the Forge, check `ae account status`, verify the account shown there, then confirm."
                            )
                        });
                    }
                    Err(error) => {
                        this.notice = Some(plain_failure(&error));
                    }
                }
                cx.emit(AccountEvent::Changed);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Starts a Forge enrollment on this machine via `ae account enroll`,
    /// then claims the printed code into this signed-in account. Local
    /// confirmation still happens on the Forge (`ae account confirm` with
    /// the expected account id; `--yes` never implies approval).
    pub fn enroll_this_forge(&mut self, cx: &mut Context<Self>) {
        if self.profile().is_none() {
            return;
        }
        self.notice = Some("Starting the Forge enrollment…".to_owned());
        cx.emit(AccountEvent::Changed);
        cx.notify();
        let session = self.session.clone();
        let http = self.http.clone();
        let base_url = self.base_url.clone();
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let enrollment = enrollment::run_ae_enroll(&base_url).await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                match (enrollment, session) {
                    (Ok(code), Some(session)) => {
                        cx.spawn(async move |this, cx| {
                            let claimed = http.claim_enrollment(&session, &code).await;
                            this.update(cx, |this, cx| {
                                if this.generation != generation {
                                    return;
                                }
                                match claimed {
                                    Ok(_) => {
                                        let email = this
                                            .profile()
                                            .map(|profile| profile.email.clone())
                                            .unwrap_or_default();
                                        this.notice = Some(format!(
                                            "Code {code} claimed for {email}. On this Forge run `ae account status`, verify the account shown there, then `ae account confirm`."
                                        ));
                                    }
                                    Err(error) => {
                                        this.notice = Some(format!(
                                            "The Forge printed code {code}, but claiming it failed: {}.",
                                            plain_failure(&error)
                                        ));
                                    }
                                }
                                cx.emit(AccountEvent::Changed);
                                cx.notify();
                            })
                            .ok();
                        })
                        .detach();
                    }
                    (Ok(code), None) => {
                        this.notice = Some(format!(
                            "The Forge printed code {code}. Sign in first, then claim it below."
                        ));
                        cx.emit(AccountEvent::Changed);
                        cx.notify();
                    }
                    (Err(error), _) => {
                        this.notice = Some(format!("Could not start the Forge enrollment: {error:#}"));
                        cx.emit(AccountEvent::Changed);
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    /// Connects an account Forge through the Relay backend: issues a fresh
    /// session-authed grant with an ephemeral `cnf`, hands it to the
    /// registered dialer, and puts the connected host into the existing
    /// inventory. Without a configured Relay route the button says so
    /// plainly with retry guidance; there is no accountless fallback.
    #[allow(
        clippy::too_many_lines,
        reason = "one grant-dial-insert flow: splitting would hide the stale-generation guards"
    )]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "callers hand over an owned machine id; the public controller API keeps String"
    )]
    pub fn connect_machine(
        &mut self,
        machine_id: String,
        core: &Entity<Core>,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let profile = match self.profile() {
            Some(profile) => profile.clone(),
            None => return,
        };
        let Some(machine) = profile
            .machines
            .iter()
            .find(|m| m.id == machine_id)
            .cloned()
        else {
            self.notice = Some("That Forge is no longer on the account.".to_owned());
            cx.emit(AccountEvent::Changed);
            cx.notify();
            return;
        };
        let Some(dialer) = self.dialer.clone() else {
            self.notice = Some(
                "The Relay route is not configured (missing Relay URL). Check the connection and try again.".to_owned(),
            );
            cx.emit(AccountEvent::Changed);
            cx.notify();
            return;
        };
        self.notice = Some(format!("Preparing the Relay route to {}…", machine.label));
        self.relay_error = None;
        cx.emit(AccountEvent::Changed);
        cx.notify();
        let http = self.http.clone();
        let core = core.clone();
        let generation = self.generation;
        let session_str = session.as_str().to_owned();
        let account_id = profile.account_id.clone();
        cx.spawn(async move |this, cx| {
            let Ok(keypair) = artisan_account::MachineKeypair::generate() else {
                this.update(cx, |this, cx| {
                    this.relay_error =
                        Some("This device could not make a connection key.".to_owned());
                    this.notice = None;
                    cx.emit(AccountEvent::Changed);
                    cx.notify();
                })
                .ok();
                return;
            };
            let public_b64u = keypair.public_b64u();
            let secret = *keypair.secret();
            let target_machine = machine.id.clone();
            let machine_id = machine.id.clone();
            let machine_label = machine.label.clone();
            let machine_key = machine.public_key.x.clone();
            let grant = run_http({
                let http = http.clone();
                async move {
                    http.issue_editor_grant(&session, &machine.id, &public_b64u)
                        .await
                }
            })
            .await;
            let request = match grant {
                Ok(grant) => RelayDialRequest {
                    account_id: profile.account_id.clone(),
                    machine_id,
                    machine_label: machine_label.clone(),
                    forge_ed25519_b64u: machine_key,
                    grant_jwt: grant.token,
                    cnf_secret: secret,
                },
                Err(error) => {
                    this.update(cx, |this, cx| {
                        if this.generation != generation {
                            return;
                        }
                        this.relay_error = Some(plain_failure(&error));
                        this.notice = None;
                        cx.emit(AccountEvent::Changed);
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };
            let task = dialer(request);
            let completed = task.await;
            // Insertion runs after the Account borrow releases: validating
            // here, inserting into Core afterwards, so the guard never reads
            // `core.account` while this Account update holds it.
            let to_insert = this
                .update(cx, |this, cx| {
                    if this.generation != generation {
                        return None;
                    }
                    // Stale account, replaced session, or revoked target must
                    // not repopulate inventory after sign-out/switch.
                    let valid = this.profile().is_some_and(|profile| {
                        profile.account_id == account_id
                            && this
                                .session_token()
                                .as_ref()
                                .is_some_and(|token| token.as_str() == session_str)
                            && profile.machines.iter().any(|machine| {
                                machine.id == target_machine && machine.revoked_at.is_none()
                            })
                    });
                    if !valid {
                        return None;
                    }
                    match completed {
                        Ok((config, client)) => {
                            this.notice =
                                Some(format!("Relay route to {machine_label} is open."));
                            this.relay_error = None;
                            cx.emit(AccountEvent::Changed);
                            cx.notify();
                            Some((config, client))
                        }
                        Err(error) => {
                            this.relay_error = Some(format!(
                                "The Relay route to {machine_label} did not open: {error:#}"
                            ));
                            cx.emit(AccountEvent::Changed);
                            cx.notify();
                            None
                        }
                    }
                })
                .ok()
                .flatten();
            if let Some((config, client)) = to_insert {
                core.update(cx, |core, cx| {
                    core.connect_with_client(&config, client, cx);
                });
            }
        })
        .detach();
    }

    /// Shows this Forge's enrollment state (`ae account status`, read-only).
    /// The operator still approves and confirms on the Forge itself.
    /// Passes the explicit Core origin so a saved custom Core is used.
    pub fn forge_status(&mut self, cx: &mut Context<Self>) {
        self.forge_status_output = Some("Checking the Forge…".to_owned());
        cx.notify();
        let base_url = self.base_url.clone();
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let output = enrollment::run_ae(&["account", "--core", &base_url, "status"]).await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.forge_status_output = Some(output);
                cx.emit(AccountEvent::Changed);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Confirms the pending enrollment on this Forge for the exact signed-in
    /// account (`--account-id` expected; `--yes` never implies approval).
    /// Only enabled after a fresh status showing a claimed enrollment, with
    /// the account email shown in native UI before explicit confirm.
    pub fn forge_confirm(&mut self, cx: &mut Context<Self>) {
        let Some(expected) = self.profile().map(|profile| profile.account_id.clone()) else {
            return;
        };
        self.forge_status_output = Some("Confirming on the Forge…".to_owned());
        cx.notify();
        let base_url = self.base_url.clone();
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let output = enrollment::run_ae(&[
                "account",
                "--core",
                &base_url,
                "confirm",
                "--yes",
                "--account-id",
                &expected,
            ])
            .await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                // Drop stale output on account switch: only apply when the
                // signed-in account still expects this binding.
                let still_expected = this
                    .profile()
                    .is_some_and(|profile| profile.account_id == expected);
                if !still_expected {
                    return;
                }
                this.forge_status_output = Some(output);
                cx.emit(AccountEvent::Changed);
                cx.notify();
                this.refresh(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Removes a Forge from the account (revokes its enrollment). Guarded by
    /// the session generation: a delayed failure from an old session (even
    /// a dead-session 401) can never clear a newer signed-in session.
    pub fn remove_machine(&mut self, machine_id: String, cx: &mut Context<Self>) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let http = self.http.clone();
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let result = run_http({
                let http = http.clone();
                async move { http.revoke_machine(&session, &machine_id).await }
            })
            .await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                match result {
                    Ok(()) => {
                        this.notice = Some("Forge removed from the account.".to_owned());
                        this.refresh(cx);
                    }
                    Err(error) => this.fail_operational(&error, cx),
                }
            })
            .ok();
        })
        .detach();
    }

    /// Dismisses the current error back to signed-out. Bumps the auth epoch
    /// so a detached earlier request can never set `Pending` afterwards.
    pub fn dismiss_error(&mut self, cx: &mut Context<Self>) {
        self.auth_epoch = self.auth_epoch.wrapping_add(1);
        self.set_phase(AccountPhase::SignedOut, cx);
    }
}


/// Runs Core HTTP on the shared Tokio runtime (never on a GPUI executor
/// thread, which has no reactor). Callers await the joined result back on
/// their own executor. A failed join is transport-level: the work never ran.
fn run_http<T>(
    fut: impl std::future::Future<Output = Result<T, NetworkError>> + Send + 'static,
) -> impl std::future::Future<Output = Result<T, NetworkError>>
where
    T: Send + 'static,
{
    use crate::runtime::runtime;
    async move {
        match runtime().spawn(fut).await {
            Err(error) => {
                tracing::warn!("account task did not run: {error}");
                Err(NetworkError::Transport)
            }
            Ok(result) => result,
        }
    }
}


fn valid_email(email: &str) -> bool {
    let email = email.trim();
    match email.split_once('@') {
        Some((local, domain)) => !local.is_empty() && domain.contains('.'),
        None => false,
    }
}

#[cfg(test)]
mod tests;
