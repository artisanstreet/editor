//! The account: magic-link sign-in, the saved session and profile, and this
//! machine's Forge enrollment. Core (the account service) is the authority;
//! only a "session invalid" answer from it signs this device out.

mod enrollment;

use std::time::Duration;

use anyhow::{Context as _, Result};
use artisan_account::network::{AccountProfile, CoreSessionClient, NetworkError, SessionToken};
use gpui::{Context, Task};
use serde::{Deserialize, Serialize};

use crate::access::{self, EntryGate, OnboardingRecord};
use crate::hosts::{EditorFiles, read_json, write_json};
use crate::services::Services;

pub use enrollment::LocalForge;

const CORE_URL_DEFAULT: &str = "https://api.artisan.st";

/// One Forge on the account, as `/v1/me` lists it.
pub type AccountMachine = artisan_account::network::MachineEntry;

#[derive(Clone, PartialEq, Eq)]
pub enum AccountPhase {
    SignedOut,
    /// A saved session from before profiles were saved, waiting for Core.
    Restoring,
    Requesting,
    /// The link is sent; waiting for the email side to approve it.
    Pending {
        request_id: String,
        request_secret: String,
        email: String,
        expires_at: String,
    },
    /// The email side approved; the user finishes here.
    EmailApproved {
        request_id: String,
        request_secret: String,
        email: String,
        totp_required: bool,
    },
    Finalizing,
    /// `verified` stays false until Core accepts the saved session this run;
    /// until then the saved profile stands in for it.
    Authenticated {
        profile: AccountProfile,
        verified: bool,
    },
    Error {
        message: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccountEvent {
    Changed,
    /// Core confirmed the session and sent a fresh profile.
    Synced,
}

impl gpui::EventEmitter<AccountEvent> for Account {}

/// `{editor-data}/account.json`.
#[derive(Default, Serialize, Deserialize)]
struct AccountFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session_token: Option<String>,
    /// The last profile Core sent, so a device that can't reach Core still
    /// opens its Forges.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    profile: Option<AccountProfile>,
}

pub struct Account {
    files: EditorFiles,
    services: Services,
    http: CoreSessionClient,
    base_url: String,
    phase: AccountPhase,
    session: Option<SessionToken>,
    onboarding: Option<OnboardingRecord>,
    /// The last `ae account status` answer; it stays when a later read fails.
    binding: Option<enrollment::AeBinding>,
    last_error: Option<String>,
    /// Request link, poll it, finalize.
    sign_in: Option<Task<()>>,
    /// Validate the session against `/v1/me`, retrying while Core is away.
    sync: Option<Task<()>>,
    /// Read, enroll, claim or confirm this machine's Forge.
    forge: Option<Task<()>>,
    /// Remove a machine or sign out everywhere.
    request: Option<Task<()>>,
}

impl Account {
    #[expect(
        clippy::expect_used,
        reason = "a reqwest client with only a timeout fails to build only when TLS itself is broken"
    )]
    pub(crate) fn new(files: EditorFiles, services: Services, cx: &mut Context<Self>) -> Self {
        let path = files.dir.join("account.json");
        let (stored, unreadable) = match read_json::<AccountFile>(&path) {
            Ok(stored) => (stored, None),
            Err(error) => (AccountFile::default(), Some(error)),
        };
        let base_url = services
            .core_url
            .clone()
            .or(stored.base_url)
            .or_else(|| std::env::var("AST_CORE_BASE_URL").ok())
            .filter(|url| !url.trim().is_empty())
            .unwrap_or_else(|| CORE_URL_DEFAULT.to_owned());
        let http = CoreSessionClient::new(&base_url).expect("Core HTTP client builds");
        let session = stored.session_token.map(SessionToken::new);
        let phase = match (unreadable, &session, stored.profile) {
            (Some(error), _, _) => AccountPhase::Error {
                message: format!("Couldn't read the saved sign-in: {error:#}"),
            },
            (None, Some(_), Some(profile)) => AccountPhase::Authenticated {
                profile,
                verified: false,
            },
            (None, Some(_), None) => AccountPhase::Restoring,
            (None, None, _) => AccountPhase::SignedOut,
        };
        let mut this = Self {
            files,
            services,
            http,
            base_url,
            phase,
            session,
            onboarding: None,
            binding: None,
            last_error: None,
            sign_in: None,
            sync: None,
            forge: None,
            request: None,
        };
        if this.profile().is_some() {
            this.load_onboarding();
            this.refresh_local_forge(cx);
        }
        if !matches!(this.phase, AccountPhase::Error { .. }) {
            this.sync(cx);
        }
        this
    }

    #[must_use]
    pub const fn phase(&self) -> &AccountPhase {
        &self.phase
    }

    #[must_use]
    pub const fn profile(&self) -> Option<&AccountProfile> {
        match &self.phase {
            AccountPhase::Authenticated { profile, .. } => Some(profile),
            AccountPhase::SignedOut
            | AccountPhase::Restoring
            | AccountPhase::Requesting
            | AccountPhase::Pending { .. }
            | AccountPhase::EmailApproved { .. }
            | AccountPhase::Finalizing
            | AccountPhase::Error { .. } => None,
        }
    }

    /// The account's Forges that are not removed.
    #[must_use]
    pub fn machines(&self) -> Vec<AccountMachine> {
        self.profile()
            .map(|profile| access::live_machines(profile).cloned().collect())
            .unwrap_or_default()
    }

    #[must_use]
    pub fn entry_gate(&self) -> EntryGate {
        access::entry_gate(&self.phase, self.onboarding.as_ref())
    }

    /// The Forge that finished setup on this device for this account.
    pub(crate) fn onboarded_machine(&self) -> Option<&str> {
        self.onboarding
            .as_ref()
            .map(|record| record.machine_id.as_str())
    }

    /// `ae` has not answered yet, so the local Forge's binding is unknown.
    pub(crate) const fn reading_local_forge(&self) -> bool {
        self.binding.is_none() && self.forge.is_some()
    }

    /// Why the last account or Forge action failed.
    #[must_use]
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub(crate) fn session(&self) -> Option<SessionToken> {
        self.session.clone()
    }

    pub(crate) const fn http(&self) -> &CoreSessionClient {
        &self.http
    }

    pub(crate) const fn services(&self) -> &Services {
        &self.services
    }

    fn changed(cx: &mut Context<Self>) {
        cx.emit(AccountEvent::Changed);
        cx.notify();
    }

    pub(crate) fn report(&mut self, error: &anyhow::Error, cx: &mut Context<Self>) {
        tracing::warn!("{error:#}");
        self.last_error = Some(format!("{error:#}"));
        Self::changed(cx);
    }

    fn fail_sign_in(&mut self, error: &NetworkError, cx: &mut Context<Self>) {
        tracing::warn!(?error, "sign-in failed");
        self.phase = AccountPhase::Error {
            message: plain_failure(error),
        };
        Self::changed(cx);
    }

    fn persist(&self) -> Result<()> {
        write_json(
            &self.files.dir.join("account.json"),
            &AccountFile {
                base_url: Some(self.base_url.clone()),
                session_token: self.session.as_ref().map(|token| token.as_str().to_owned()),
                profile: self.profile().cloned(),
            },
        )
    }

    fn load_onboarding(&mut self) {
        let Some(account_id) = self.profile().map(|profile| profile.account_id.clone()) else {
            self.onboarding = None;
            return;
        };
        match access::load_onboarding(&self.files.dir, &self.base_url, &account_id) {
            Ok(record) => self.onboarding = record,
            Err(error) => {
                self.onboarding = None;
                self.last_error = Some(format!("{error:#}"));
            }
        }
    }

    /// Records that setup finished on `machine_id`; the gate opens.
    pub(crate) fn complete_onboarding(
        &mut self,
        machine_id: String,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        let account_id = self
            .profile()
            .map(|profile| profile.account_id.clone())
            .context("sign in first")?;
        let record = OnboardingRecord {
            account_id,
            base_url: self.base_url.clone(),
            machine_id,
        };
        access::save_onboarding(&self.files.dir, &record)?;
        self.onboarding = Some(record);
        Self::changed(cx);
        Ok(())
    }

    fn signed_in(&mut self, profile: AccountProfile, cx: &mut Context<Self>) {
        let switched = self
            .profile()
            .is_none_or(|current| current.account_id != profile.account_id);
        self.phase = AccountPhase::Authenticated {
            profile,
            verified: true,
        };
        if switched {
            self.load_onboarding();
        }
        if let Err(error) = self.persist() {
            self.last_error = Some(format!("{error:#}"));
        }
        if self.binding.is_none() && self.forge.is_none() {
            self.refresh_local_forge(cx);
        }
        cx.emit(AccountEvent::Synced);
        Self::changed(cx);
    }

    /// Forgets the session on this device. Hosts, Forge data, enrollment
    /// and onboarding stay on disk for the next sign-in.
    fn clear_session(&mut self, cx: &mut Context<Self>) {
        self.session = None;
        self.phase = AccountPhase::SignedOut;
        self.onboarding = None;
        self.sign_in = None;
        self.sync = None;
        self.forge = None;
        self.request = None;
        if let Err(error) = self.persist() {
            self.last_error = Some(format!("{error:#}"));
        }
        Self::changed(cx);
    }

    // -- sign-in ------------------------------------------------------

    /// Emails a sign-in link and waits for it to be approved.
    pub fn request_link(&mut self, email: &str, cx: &mut Context<Self>) {
        let http = self.http.clone();
        let device_name = crate::device_name();
        let email = email.trim().to_owned();
        self.phase = AccountPhase::Requesting;
        self.last_error = None;
        Self::changed(cx);
        self.sign_in = Some(cx.spawn(async move |this, cx| {
            let requested = run_http({
                let (http, email) = (http.clone(), email.clone());
                async move { http.magic_request(&email, &device_name).await }
            })
            .await;
            let Ok(Some((request_id, secret))) = this.update(cx, |this, cx| match requested {
                Ok(requested) => {
                    this.phase = AccountPhase::Pending {
                        request_id: requested.request_id.clone(),
                        request_secret: requested.request_secret.clone(),
                        email,
                        expires_at: requested.expires_at,
                    };
                    Self::changed(cx);
                    Some((requested.request_id, requested.request_secret))
                }
                Err(error) => {
                    this.fail_sign_in(&error, cx);
                    None
                }
            }) else {
                return;
            };
            loop {
                cx.background_executor().timer(Duration::from_secs(3)).await;
                let status = run_http({
                    let (http, request_id, secret) =
                        (http.clone(), request_id.clone(), secret.clone());
                    async move { http.magic_status(&request_id, &secret).await }
                })
                .await;
                let done = this.update(cx, |this, cx| match status {
                    // Keep waiting through a network blip.
                    Err(NetworkError::Transport) => false,
                    Err(error) => {
                        this.fail_sign_in(&error, cx);
                        true
                    }
                    Ok(status) if status.status == "pending" => false,
                    Ok(status) if status.status == "email_approved" => {
                        this.phase = AccountPhase::EmailApproved {
                            request_id: request_id.clone(),
                            request_secret: secret.clone(),
                            email: status.email,
                            totp_required: status.totp_required,
                        };
                        Self::changed(cx);
                        true
                    }
                    Ok(_) => {
                        this.phase = AccountPhase::Error {
                            message: "That sign-in request expired. Request a new link.".to_owned(),
                        };
                        Self::changed(cx);
                        true
                    }
                });
                // Err: the Account is gone.
                if done.unwrap_or(true) {
                    return;
                }
            }
        }));
    }

    /// Finishes an approved sign-in, with the second factor when the
    /// account has one.
    pub fn finish_sign_in(
        &mut self,
        totp_code: Option<String>,
        recovery_code: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let AccountPhase::EmailApproved {
            request_id,
            request_secret,
            ..
        } = &self.phase
        else {
            return;
        };
        let (request_id, secret) = (request_id.clone(), request_secret.clone());
        let http = self.http.clone();
        self.phase = AccountPhase::Finalizing;
        Self::changed(cx);
        self.sign_in = Some(cx.spawn(async move |this, cx| {
            let finalized = run_http({
                let http = http.clone();
                async move {
                    http.magic_finalize(
                        &request_id,
                        &secret,
                        totp_code.as_deref().filter(|code| !code.trim().is_empty()),
                        recovery_code
                            .as_deref()
                            .filter(|code| !code.trim().is_empty()),
                    )
                    .await
                }
            })
            .await;
            // Err: the Account is gone.
            this.update(cx, |this, cx| match finalized {
                Ok((_, token)) => {
                    this.session = Some(token);
                    this.phase = AccountPhase::Restoring;
                    if let Err(error) = this.persist() {
                        this.last_error = Some(format!("{error:#}"));
                    }
                    Self::changed(cx);
                    this.sync(cx);
                }
                Err(error) => this.fail_sign_in(&error, cx),
            })
            .ok();
        }));
    }

    pub fn dismiss_error(&mut self, cx: &mut Context<Self>) {
        self.sign_in = None;
        self.phase = AccountPhase::SignedOut;
        Self::changed(cx);
    }

    // -- session ------------------------------------------------------

    /// Checks the session with Core now, retrying with backoff while Core
    /// can't be reached.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.sync(cx);
    }

    fn sync(&mut self, cx: &Context<Self>) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let http = self.http.clone();
        self.sync = Some(cx.spawn(async move |this, cx| {
            let mut delay = Duration::from_secs(2);
            loop {
                let profile = run_http({
                    let (http, session) = (http.clone(), session.clone());
                    async move { http.me(&session).await }
                })
                .await;
                let retry = this.update(cx, |this, cx| match profile {
                    Ok(profile) => {
                        this.signed_in(profile, cx);
                        false
                    }
                    Err(error) if is_session_invalid(&error) => {
                        tracing::warn!(?error, "Core ended the session");
                        this.clear_session(cx);
                        false
                    }
                    Err(error) => {
                        tracing::warn!(?error, "Core unreachable; keeping the saved session");
                        if let AccountPhase::Authenticated { verified, .. } = &mut this.phase {
                            *verified = false;
                        }
                        Self::changed(cx);
                        true
                    }
                });
                // Err: the Account is gone.
                if !retry.unwrap_or(false) {
                    return;
                }
                cx.background_executor().timer(delay).await;
                delay = (delay * 2).min(Duration::from_secs(60));
            }
        }));
    }

    pub fn sign_out(&mut self, cx: &mut Context<Self>) {
        if let Some(session) = self.session.clone() {
            let http = self.http.clone();
            crate::runtime::runtime().spawn(async move {
                if let Err(error) = http.revoke_current(&session).await {
                    tracing::warn!("couldn't revoke the session at Core: {error:#}");
                }
            });
        }
        self.clear_session(cx);
    }

    /// Ends every session on the account, this one included.
    pub fn sign_out_everywhere(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let http = self.http.clone();
        self.request = Some(cx.spawn(async move |this, cx| {
            let result = run_http(async move { http.revoke_all_sessions(&session).await }).await;
            // Err: the Account is gone.
            this.update(cx, |this, cx| match result {
                Ok(()) => this.clear_session(cx),
                Err(error) if is_session_invalid(&error) => this.clear_session(cx),
                Err(error) => {
                    this.report(
                        &anyhow::Error::new(error).context("sign out everywhere"),
                        cx,
                    );
                }
            })
            .ok();
        }));
    }

    /// Removes a Forge from the account.
    pub fn remove_machine(&mut self, machine_id: String, cx: &mut Context<Self>) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let http = self.http.clone();
        self.request = Some(cx.spawn(async move |this, cx| {
            let result =
                run_http(async move { http.revoke_machine(&session, &machine_id).await }).await;
            // Err: the Account is gone.
            this.update(cx, |this, cx| match result {
                Ok(()) => this.sync(cx),
                Err(error) if is_session_invalid(&error) => this.clear_session(cx),
                Err(error) => {
                    this.report(&anyhow::Error::new(error).context("remove the Forge"), cx);
                }
            })
            .ok();
        }));
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn sign_in_for_tests(
        &mut self,
        profile: AccountProfile,
        machine_id: String,
        cx: &mut Context<Self>,
    ) {
        self.session = Some(SessionToken::new("test-session".to_owned()));
        self.onboarding = Some(OnboardingRecord {
            account_id: profile.account_id.clone(),
            base_url: self.base_url.clone(),
            machine_id,
        });
        self.binding = Some(enrollment::AeBinding::Unenrolled);
        self.phase = AccountPhase::Authenticated {
            profile,
            verified: true,
        };
        Self::changed(cx);
    }
}

/// Core says this session is over: expired, revoked, or never valid.
pub fn is_session_invalid(error: &NetworkError) -> bool {
    matches!(error, NetworkError::Core { code, .. } if matches!(code.as_str(), "unauthorized" | "gone"))
}

/// User-facing text for a failed sign-in step; never Core's own text.
fn plain_failure(error: &NetworkError) -> String {
    match error {
        NetworkError::Core { code, .. } => match code.as_str() {
            "bad_request" => "Core could not use that request. Check the email address.",
            "unauthorized" => "That sign-in link is invalid or already used.",
            "forbidden" => "The email link is not approved yet, or the second factor is missing.",
            "gone" => "That sign-in request expired. Request a new link.",
            "rate_limited" => "Too many attempts. Wait a while and try again.",
            _ => "Core could not do that right now. Try again in a bit.",
        },
        NetworkError::Transport => "Could not reach Core. Check the connection and try again.",
    }
    .to_owned()
}

/// Runs Core HTTP on the network runtime; GPUI's executors have no reactor.
pub async fn run_http<T: Send + 'static>(
    future: impl std::future::Future<Output = Result<T, NetworkError>> + Send + 'static,
) -> Result<T, NetworkError> {
    match tokio_util::task::AbortOnDropHandle::new(crate::runtime::runtime().spawn(future)).await {
        Ok(result) => result,
        Err(error) => {
            tracing::warn!("Core request task did not finish: {error}");
            Err(NetworkError::Transport)
        }
    }
}
