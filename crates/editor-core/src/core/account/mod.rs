mod enrollment;

use crate::core::{
    access::{self, EntryGate, OnboardingRecord},
    config_file,
    delivery::{AdmissionOrder, Delivery},
    hosts::EditorFiles,
    runtime::runtime,
    services::Services,
};
pub use artisan_account::browser::Provider;
use artisan_account::browser::{self, Callback, Handoff, Problem, ReceiveError};
use artisan_account::network::{AccountProfile, CoreSessionClient, NetworkError, SessionToken};
pub use enrollment::LocalForge;
use enrollment::{AeBinding, BindForgeError};
use serde::{Deserialize, Serialize};
use std::{num::NonZeroUsize, time::Duration};

const CORE_URL_OVERRIDE: &str = "AST_CORE_BASE_URL";
const BROWSER_TIMEOUT: Duration = Duration::from_mins(10);
pub const AUTH_ORIGIN: &str = "https://auth.artisan.st";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccountLink {
    Terms,
    Privacy,
    Recover,
}

impl AccountLink {
    #[must_use]
    pub fn url(self) -> String {
        let path = match self {
            Self::Terms => "/terms",
            Self::Privacy => "/privacy",
            Self::Recover => "/recover",
        };
        format!("{AUTH_ORIGIN}{path}")
    }
}
use tokio_util::task::AbortOnDropHandle;

pub type AccountMachine = artisan_account::network::MachineEntry;

#[derive(Clone, PartialEq, Eq)]
pub enum AccountPhase {
    SignedOut,
    Restoring,
    Requesting,
    Browser {
        provider: Provider,
    },
    Pending {
        request_id: String,
        request_secret: String,
        email: String,
        expires_at: String,
    },
    EmailApproved {
        approval: Approval,
        rejected: Option<Factor>,
    },
    Finalizing {
        approval: Approval,
        factor: Option<SecondFactor>,
    },
    Authenticated {
        profile: AccountProfile,
        verified: bool,
    },
    Error {
        message: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Approval {
    pub request_id: String,
    pub request_secret: String,
    pub email: String,
    pub totp_required: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Factor {
    Authenticator,
    Recovery,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SecondFactor {
    Authenticator(String),
    Recovery(String),
}

impl SecondFactor {
    #[must_use]
    pub fn parse(input: &str) -> Option<Self> {
        let code = input.trim();
        match (
            code.is_empty(),
            code.len() == 6 && code.bytes().all(|b| b.is_ascii_digit()),
        ) {
            (true, _) => None,
            (false, true) => Some(Self::Authenticator(code.to_owned())),
            (false, false) => Some(Self::Recovery(code.to_owned())),
        }
    }

    #[must_use]
    pub const fn kind(&self) -> Factor {
        match self {
            Self::Authenticator(_) => Factor::Authenticator,
            Self::Recovery(_) => Factor::Recovery,
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum StartBrowserSignInError {
    #[error("make the browser hand-off secrets")]
    Handoff(#[source] getrandom::Error),
    #[error("listen for the browser's redirect")]
    Listen(#[source] std::io::Error),
}

#[derive(Debug, thiserror::Error)]
enum AccountRequestError {
    #[error("remove the Forge")]
    RemoveForge(#[source] NetworkError),
    #[error("sign out everywhere")]
    SignOutEverywhere(#[source] NetworkError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccountEvent {
    Changed,
    Synced,
}

#[derive(Default, Serialize, Deserialize)]
struct AccountFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    profile: Option<AccountProfile>,
}

enum SignInStatus {
    Waiting,
    Approved { email: String, totp_required: bool },
    Expired,
}

enum Message {
    Requested(
        u64,
        String,
        Result<(String, String, String), NetworkError>,
        tokio::sync::oneshot::Sender<()>,
    ),
    Status(
        u64,
        String,
        String,
        Result<SignInStatus, NetworkError>,
        tokio::sync::oneshot::Sender<()>,
    ),
    Finalized(u64, Result<SessionToken, NetworkError>),
    BrowserFailed(u64, Provider, BrowserFailure),
    Synced(
        u64,
        Result<AccountProfile, NetworkError>,
        tokio::sync::oneshot::Sender<()>,
    ),
    Binding(u64, Result<AeBinding, BindForgeError>, bool),
    Request(u64, Result<(), NetworkError>, bool),
}

pub(crate) struct AccountEvents {
    applied: Option<tokio::sync::oneshot::Sender<()>>,
}
impl Drop for AccountEvents {
    fn drop(&mut self) {
        if let Some(applied) = self.applied.take()
            && applied.send(()).is_err()
        {
            tracing::debug!("the account task stopped waiting for its transition to apply");
        }
    }
}

pub struct Account {
    files: EditorFiles,
    services: Services,
    http: CoreSessionClient,
    base_url: String,
    phase: AccountPhase,
    session: Option<SessionToken>,
    onboarding: Option<OnboardingRecord>,
    binding: Option<AeBinding>,
    last_error: Option<crate::core::Problem>,
    delivery: Delivery<Message>,
    sign_in: Option<AbortOnDropHandle<()>>,
    sync: Option<AbortOnDropHandle<()>>,
    forge: Option<AbortOnDropHandle<()>>,
    request: Option<AbortOnDropHandle<()>>,
    sign_generation: u64,
    sync_generation: u64,
    forge_generation: u64,
    request_generation: u64,
    events: crate::core::store::Effects,
}

fn core_url_override() -> Option<String> {
    match std::env::var(CORE_URL_OVERRIDE) {
        Ok(url) => Some(url),
        Err(std::env::VarError::NotPresent) => None,
        Err(error @ std::env::VarError::NotUnicode(_)) => {
            tracing::warn!(%error, "read {CORE_URL_OVERRIDE}; using the saved or default Core URL");
            None
        }
    }
}

impl Account {
    pub(crate) fn new(
        files: EditorFiles,
        services: Services,
        wake: aqua::mailbox::Sender<()>,
        order: AdmissionOrder,
        effects: crate::core::store::Effects,
    ) -> Self {
        let (stored, unreadable) =
            match config_file::read::<AccountFile>(&files.dir.join("account.json")) {
                Ok(stored) => (stored, None),
                Err(error) => (AccountFile::default(), Some(error)),
            };
        let base_url = services
            .core_url
            .clone()
            .or(stored.base_url)
            .or_else(core_url_override)
            .filter(|url| !url.trim().is_empty())
            .unwrap_or_else(|| "https://api.artisan.st".to_owned());
        let http = CoreSessionClient::new(&base_url).expect(
            "invariant: a rustls client with fixed settings and bundled roots always builds",
        );
        let session = stored.session_token.map(SessionToken::new);
        let phase = match (unreadable, &session, stored.profile) {
            (Some(error), _, _) => {
                tracing::warn!(
                    error = %::diagnostics::chain(&error),
                    "couldn't read the saved sign-in"
                );
                AccountPhase::Error {
                    message: "Couldn't read the sign-in saved on this device. Sign in again."
                        .to_owned(),
                }
            }
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
            delivery: Delivery::with_order(
                NonZeroUsize::new(64)
                    .expect("invariant: the account delivery capacity is non-zero"),
                wake,
                order,
            ),
            sign_in: None,
            sync: None,
            forge: None,
            request: None,
            sign_generation: 0,
            sync_generation: 0,
            forge_generation: 0,
            request_generation: 0,
            events: effects,
        };
        if this.profile().is_some() {
            this.load_onboarding();
            this.refresh_local_forge();
        }
        if !matches!(this.phase, AccountPhase::Error { .. }) {
            this.refresh();
        }
        this
    }

    pub fn phase(&self) -> &AccountPhase {
        &self.phase
    }
    pub fn profile(&self) -> Option<&AccountProfile> {
        match &self.phase {
            AccountPhase::Authenticated { profile, .. } => Some(profile),
            AccountPhase::SignedOut
            | AccountPhase::Restoring
            | AccountPhase::Requesting
            | AccountPhase::Browser { .. }
            | AccountPhase::Pending { .. }
            | AccountPhase::EmailApproved { .. }
            | AccountPhase::Finalizing { .. }
            | AccountPhase::Error { .. } => None,
        }
    }
    pub fn machines(&self) -> Vec<AccountMachine> {
        self.profile()
            .map(|profile| access::live_machines(profile).cloned().collect())
            .unwrap_or_default()
    }
    pub(crate) fn entry_gate(&self) -> EntryGate {
        access::entry_gate(&self.phase)
    }
    pub const fn last_error(&self) -> Option<&crate::core::Problem> {
        self.last_error.as_ref()
    }
    pub fn local_forge(&self) -> LocalForge {
        match (&self.binding, self.profile()) {
            (Some(binding), Some(profile)) => enrollment::local_forge(binding, profile),
            _ => LocalForge::Unknown,
        }
    }
    pub(crate) fn onboarded_machine(&self) -> Option<&str> {
        self.onboarding
            .as_ref()
            .map(|record| record.machine_id.as_str())
    }
    pub fn reading_local_forge(&self) -> bool {
        self.binding.is_none() && self.forge.is_some()
    }
    pub(crate) fn session(&self) -> Option<SessionToken> {
        self.session.clone()
    }
    pub(crate) fn http(&self) -> &CoreSessionClient {
        &self.http
    }
    pub(crate) fn services(&self) -> &Services {
        &self.services
    }
    fn changed(&self) {
        self.events.account(AccountEvent::Changed);
    }
    fn persist(&self) -> Result<(), config_file::WriteConfigError> {
        config_file::write(
            &self.files.dir.join("account.json"),
            &AccountFile {
                base_url: Some(self.base_url.clone()),
                session_token: self.session.as_ref().map(|token| token.as_str().to_owned()),
                profile: self.profile().cloned(),
            },
        )
    }
    fn load_onboarding(&mut self) {
        let Some(id) = self.profile().map(|p| p.account_id.clone()) else {
            self.onboarding = None;
            return;
        };
        match access::load_onboarding(&self.files.dir, &self.base_url, &id) {
            Ok(record) => self.onboarding = record,
            Err(error) => {
                self.onboarding = None;
                self.last_error = Some(crate::core::Problem::caused_by(
                    "Couldn't read your account's setup progress.",
                    &error,
                ));
            }
        }
    }
    pub(crate) fn report(&mut self, headline: &str, error: &dyn std::error::Error) {
        let problem = crate::core::Problem::caused_by(headline, error);
        tracing::warn!(error = ?problem.detail(), "account step failed");
        self.last_error = Some(problem);
        self.changed();
    }
    fn clear_session(&mut self) {
        self.session = None;
        self.phase = AccountPhase::SignedOut;
        self.onboarding = None;
        self.sign_in = None;
        self.sync = None;
        self.forge = None;
        self.request = None;
        self.sign_generation += 1;
        self.sync_generation += 1;
        self.forge_generation += 1;
        self.request_generation += 1;
        if let Err(error) = self.persist() {
            self.last_error = Some(crate::core::Problem::caused_by(
                "Couldn't save your sign-in on this device.",
                &error,
            ));
        }
        self.changed();
    }
    fn fail_sign_in(&mut self, error: &NetworkError) {
        self.phase = AccountPhase::Error {
            message: plain_failure(error),
        };
        self.changed();
    }
    pub fn request_link(&mut self, email: &str, device_name: String) {
        self.sign_generation += 1;
        let generation = self.sign_generation;
        let http = self.http.clone();
        let email = email.trim().to_owned();
        let sender = self.delivery.sender();
        self.phase = AccountPhase::Requesting;
        self.last_error = None;
        self.changed();
        self.sign_in = Some(AbortOnDropHandle::new(runtime().spawn(async move {
            let result = http
                .magic_request(&email, &device_name)
                .await
                .map(|response| {
                    (
                        response.request_id,
                        response.request_secret,
                        response.expires_at,
                    )
                });
            let request = result
                .as_ref()
                .ok()
                .map(|(id, secret, _)| (id.clone(), secret.clone()));
            let (applied, acknowledged) = tokio::sync::oneshot::channel();
            if sender
                .send(Message::Requested(generation, email, result, applied))
                .await
                .is_err()
            {
                return;
            }
            let Some((id, secret)) = request else {
                return;
            };
            if acknowledged.await.is_err() {
                return;
            }
            loop {
                tokio::time::sleep(Duration::from_secs(3)).await;
                let result = http.magic_status(&id, &secret).await.map(|status| {
                    match status.status.as_str() {
                        "pending" => SignInStatus::Waiting,
                        "email_approved" => SignInStatus::Approved {
                            email: status.email,
                            totp_required: status.totp_required,
                        },
                        _ => SignInStatus::Expired,
                    }
                });
                let done = !matches!(&result, Ok(SignInStatus::Waiting))
                    && !result.as_ref().is_err_and(NetworkError::is_unreachable);
                let (applied, acknowledged) = tokio::sync::oneshot::channel();
                if sender
                    .send(Message::Status(
                        generation,
                        id.clone(),
                        secret.clone(),
                        result,
                        applied,
                    ))
                    .await
                    .is_err()
                {
                    return;
                }
                if done || acknowledged.await.is_err() {
                    return;
                }
            }
        })));
    }

    pub fn sign_in_with(&mut self, provider: Provider, auth_origin: &str) -> Option<String> {
        let started = Handoff::generate()
            .map_err(StartBrowserSignInError::Handoff)
            .and_then(|handoff| {
                browser::bind()
                    .map(|bound| (handoff, bound))
                    .map_err(StartBrowserSignInError::Listen)
            });
        let (handoff, (listener, port)) = match started {
            Ok(started) => started,
            Err(error) => {
                tracing::warn!(
                    "couldn't start browser sign-in: {}",
                    ::diagnostics::chain(&error)
                );
                self.phase = AccountPhase::Error {
                    message: format!("Couldn't start {} sign-in. Try again.", provider.name()),
                };
                self.changed();
                return None;
            }
        };
        self.sign_generation += 1;
        let generation = self.sign_generation;
        let http = self.http.clone();
        let sender = self.delivery.sender();
        let origin = auth_origin.to_owned();
        let url = handoff.start_url(auth_origin, provider, port);
        self.phase = AccountPhase::Browser { provider };
        self.last_error = None;
        self.changed();
        self.sign_in = Some(AbortOnDropHandle::new(runtime().spawn(async move {
            let callback =
                browser::receive(listener, handoff.state(), &origin, BROWSER_TIMEOUT).await;
            let message = match callback {
                Ok(Callback::Code(code)) => Message::Finalized(
                    generation,
                    http.desktop_redeem(&code, handoff.verifier())
                        .await
                        .map(|(_, token)| token),
                ),
                Ok(Callback::Problem(problem)) => {
                    Message::BrowserFailed(generation, provider, BrowserFailure::Problem(problem))
                }
                Err(error) => {
                    Message::BrowserFailed(generation, provider, BrowserFailure::Receive(error))
                }
            };
            sender.offer(message).await;
        })));
        Some(url)
    }

    pub fn finish_sign_in(&mut self, factor: Option<SecondFactor>) {
        let AccountPhase::EmailApproved { approval, .. } = &self.phase else {
            return;
        };
        let approval = approval.clone();
        let (id, secret) = (approval.request_id.clone(), approval.request_secret.clone());
        self.sign_generation += 1;
        let generation = self.sign_generation;
        let http = self.http.clone();
        let sender = self.delivery.sender();
        let (totp, recovery) = match &factor {
            Some(SecondFactor::Authenticator(code)) => (Some(code.clone()), None),
            Some(SecondFactor::Recovery(code)) => (None, Some(code.clone())),
            None => (None, None),
        };
        self.phase = AccountPhase::Finalizing { approval, factor };
        self.changed();
        self.sign_in = Some(AbortOnDropHandle::new(runtime().spawn(async move {
            let result = http
                .magic_finalize(&id, &secret, totp.as_deref(), recovery.as_deref())
                .await
                .map(|(_, token)| token);
            sender.offer(Message::Finalized(generation, result)).await;
        })));
    }
    pub fn clear_rejection(&mut self) {
        if let AccountPhase::EmailApproved { rejected, .. } = &mut self.phase
            && rejected.take().is_some()
        {
            self.changed();
        }
    }
    pub fn dismiss_error(&mut self) {
        self.sign_in = None;
        self.sign_generation += 1;
        self.phase = AccountPhase::SignedOut;
        self.changed();
    }
    pub fn refresh(&mut self) {
        let Some(session) = self.session.clone() else {
            return;
        };
        self.sync_generation += 1;
        let generation = self.sync_generation;
        let http = self.http.clone();
        let sender = self.delivery.sender();
        self.sync = Some(AbortOnDropHandle::new(runtime().spawn(async move {
            let mut delay = Duration::from_secs(2);
            loop {
                let result = http.me(&session).await;
                let retry = matches!(&result, Err(error) if !is_session_invalid(error));
                let (applied, acknowledged) = tokio::sync::oneshot::channel();
                if sender
                    .send(Message::Synced(generation, result, applied))
                    .await
                    .is_err()
                {
                    return;
                }
                if !retry || acknowledged.await.is_err() {
                    return;
                }
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_secs(60));
            }
        })));
    }
    pub fn sign_out(&mut self) {
        if let Some(session) = self.session.clone() {
            let http = self.http.clone();
            runtime().spawn(async move {
                if let Err(error) = http.revoke_current(&session).await {
                    tracing::warn!(
                        "couldn't revoke the session at Core: {}",
                        ::diagnostics::chain(&error)
                    );
                }
            });
        }
        self.clear_session();
    }
    pub fn sign_out_everywhere(&mut self) {
        self.request_action(None);
    }
    pub fn remove_machine(&mut self, machine_id: String) {
        self.request_action(Some(machine_id));
    }
    fn request_action(&mut self, machine: Option<String>) {
        let Some(session) = self.session.clone() else {
            return;
        };
        self.request_generation += 1;
        let generation = self.request_generation;
        let http = self.http.clone();
        let sender = self.delivery.sender();
        self.request = Some(AbortOnDropHandle::new(runtime().spawn(async move {
            let remove = machine.is_some();
            let result = match machine {
                Some(id) => http.revoke_machine(&session, &id).await,
                None => http.revoke_all_sessions(&session).await,
            };
            sender
                .offer(Message::Request(generation, result, remove))
                .await;
        })));
    }
    pub fn refresh_local_forge(&mut self) {
        self.forge_generation += 1;
        let generation = self.forge_generation;
        let (ae, core, sender) = (
            self.services.ae.clone(),
            self.base_url.clone(),
            self.delivery.sender(),
        );
        self.forge = Some(AbortOnDropHandle::new(runtime().spawn(async move {
            let result = enrollment::read_status(&ae, &core).await;
            sender
                .offer(Message::Binding(generation, result, false))
                .await;
        })));
    }
    pub fn enroll_this_forge(&mut self) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let replace = matches!(self.local_forge(), LocalForge::Revoked { .. });
        self.forge_generation += 1;
        let generation = self.forge_generation;
        let (ae, core_url, http, sender) = (
            self.services.ae.clone(),
            self.base_url.clone(),
            self.http.clone(),
            self.delivery.sender(),
        );
        self.last_error = None;
        self.forge = Some(AbortOnDropHandle::new(runtime().spawn(async move {
            let result = async {
                let mut args = vec!["account", "--core_url", &core_url, "enroll", "--json"];
                if replace {
                    args.push("--replace");
                }
                let printed = ae.run(&args).await.map_err(BindForgeError::Enroll)?;
                let enrollment: enrollment::Enrollment =
                    serde_json::from_str(&printed).map_err(BindForgeError::ReadEnrollment)?;
                let code = artisan_account::normalise_code(&enrollment.code);
                http.claim_enrollment(&session, &code)
                    .await
                    .map_err(BindForgeError::Claim)?;
                enrollment::read_status(&ae, &core_url).await
            }
            .await;
            sender
                .offer(Message::Binding(generation, result, false))
                .await;
        })));
    }
    pub fn claim_code(&mut self, code: &str) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let code = artisan_account::normalise_code(code);
        self.forge_generation += 1;
        let generation = self.forge_generation;
        let (ae, core_url, http, sender) = (
            self.services.ae.clone(),
            self.base_url.clone(),
            self.http.clone(),
            self.delivery.sender(),
        );
        self.last_error = None;
        self.forge = Some(AbortOnDropHandle::new(runtime().spawn(async move {
            let result = async {
                http.claim_enrollment(&session, &code)
                    .await
                    .map_err(BindForgeError::Claim)?;
                enrollment::read_status(&ae, &core_url).await
            }
            .await;
            sender
                .offer(Message::Binding(generation, result, false))
                .await;
        })));
    }
    pub fn confirm_forge(&mut self) {
        let Some(account_id) = self.profile().map(|profile| profile.account_id.clone()) else {
            return;
        };
        self.forge_generation += 1;
        let generation = self.forge_generation;
        let (ae, core, sender) = (
            self.services.ae.clone(),
            self.base_url.clone(),
            self.delivery.sender(),
        );
        self.last_error = None;
        self.forge = Some(AbortOnDropHandle::new(runtime().spawn(async move {
            let result = async {
                ae.run(&[
                    "account",
                    "--core",
                    &core,
                    "confirm",
                    "--yes",
                    "--account-id",
                    &account_id,
                ])
                .await
                .map_err(|source| BindForgeError::Confirm {
                    account: account_id.clone(),
                    source,
                })?;
                enrollment::read_status(&ae, &core).await
            }
            .await;
            sender
                .offer(Message::Binding(generation, result, true))
                .await;
        })));
    }
    pub(crate) fn peek_order(&mut self) -> Option<u64> {
        self.delivery.peek_order()
    }
    pub(crate) fn admitted(&self) -> usize {
        self.delivery.admitted()
    }
    pub(crate) fn drain(&mut self) -> AccountEvents {
        let applied = self
            .delivery
            .drain_one()
            .and_then(|message| self.apply(message));
        AccountEvents { applied }
    }
    fn apply(&mut self, message: Message) -> Option<tokio::sync::oneshot::Sender<()>> {
        match message {
            Message::Requested(generation, email, result, applied)
                if generation == self.sign_generation =>
            {
                self.requested(email, result);
                Some(applied)
            }
            Message::Status(generation, id, secret, result, applied)
                if generation == self.sign_generation =>
            {
                self.status(id, secret, result);
                Some(applied)
            }
            Message::Finalized(generation, result) if generation == self.sign_generation => {
                self.finalized(result);
                None
            }
            Message::BrowserFailed(generation, provider, failure)
                if generation == self.sign_generation =>
            {
                self.sign_in = None;
                self.phase = AccountPhase::Error {
                    message: failure.message(provider),
                };
                self.changed();
                None
            }
            Message::Synced(generation, result, applied) if generation == self.sync_generation => {
                self.synced(result);
                Some(applied)
            }
            Message::Binding(generation, result, refresh)
                if generation == self.forge_generation =>
            {
                self.bound(result, refresh);
                None
            }
            Message::Request(generation, result, remove)
                if generation == self.request_generation =>
            {
                self.request_finished(result, remove);
                None
            }
            Message::Requested(..)
            | Message::Status(..)
            | Message::Finalized(..)
            | Message::BrowserFailed(..)
            | Message::Synced(..)
            | Message::Binding(..)
            | Message::Request(..) => None,
        }
    }
    fn requested(&mut self, email: String, result: Result<(String, String, String), NetworkError>) {
        match result {
            Ok((request_id, request_secret, expires_at)) => {
                self.phase = AccountPhase::Pending {
                    request_id,
                    request_secret,
                    email,
                    expires_at,
                };
                self.changed();
            }
            Err(error) => self.fail_sign_in(&error),
        }
    }
    fn status(&mut self, id: String, secret: String, result: Result<SignInStatus, NetworkError>) {
        match result {
            Err(error) if error.is_unreachable() => {}
            Err(error) => self.fail_sign_in(&error),
            Ok(SignInStatus::Waiting) => {}
            Ok(SignInStatus::Approved {
                email,
                totp_required,
            }) => {
                self.phase = AccountPhase::EmailApproved {
                    approval: Approval {
                        request_id: id,
                        request_secret: secret,
                        email,
                        totp_required,
                    },
                    rejected: None,
                };
                self.changed();
            }
            Ok(SignInStatus::Expired) => {
                self.phase = AccountPhase::Error {
                    message: "That sign-in request expired. Request a new link.".to_owned(),
                };
                self.changed();
            }
        }
    }
    fn finalized(&mut self, result: Result<SessionToken, NetworkError>) {
        match result {
            Ok(token) => {
                self.session = Some(token);
                self.phase = AccountPhase::Restoring;
                if let Err(error) = self.persist() {
                    self.last_error = Some(crate::core::Problem::caused_by(
                        "Couldn't save your sign-in on this device.",
                        &error,
                    ));
                }
                self.changed();
                self.refresh();
            }
            Err(error) => match (&self.phase, &error) {
                (
                    AccountPhase::Finalizing {
                        approval,
                        factor: Some(factor),
                    },
                    NetworkError::Core { code, .. },
                ) if code == "forbidden" => {
                    self.phase = AccountPhase::EmailApproved {
                        approval: approval.clone(),
                        rejected: Some(factor.kind()),
                    };
                    self.changed();
                }
                _ => self.fail_sign_in(&error),
            },
        }
    }
    fn synced(&mut self, result: Result<AccountProfile, NetworkError>) {
        match result {
            Ok(profile) => {
                let switched = self
                    .profile()
                    .is_none_or(|old| old.account_id != profile.account_id);
                self.phase = AccountPhase::Authenticated {
                    profile,
                    verified: true,
                };
                if switched {
                    self.load_onboarding();
                }
                if let Err(error) = self.persist() {
                    self.last_error = Some(crate::core::Problem::caused_by(
                        "Couldn't save your sign-in on this device.",
                        &error,
                    ));
                }
                if self.binding.is_none() && self.forge.is_none() {
                    self.refresh_local_forge();
                }
                self.events.account(AccountEvent::Synced);
                self.changed();
            }
            Err(error) if is_session_invalid(&error) => self.clear_session(),
            Err(error) => {
                tracing::warn!(?error, "Core unreachable; keeping the saved session");
                if let AccountPhase::Authenticated { verified, .. } = &mut self.phase {
                    *verified = false;
                }
                self.changed();
            }
        }
    }
    fn bound(&mut self, result: Result<AeBinding, BindForgeError>, refresh: bool) {
        self.forge = None;
        match result {
            Ok(binding) => {
                self.binding = Some(binding);
                self.changed();
                if refresh {
                    self.refresh();
                }
            }
            Err(error) => self.report("Couldn't connect this Forge to your account.", &error),
        }
    }
    fn request_finished(&mut self, result: Result<(), NetworkError>, remove: bool) {
        self.request = None;
        match result {
            Ok(()) if remove => self.refresh(),
            Ok(()) => self.clear_session(),
            Err(error) if is_session_invalid(&error) => self.clear_session(),
            Err(source) if remove => self.report(
                "Couldn't remove this Forge from your account.",
                &AccountRequestError::RemoveForge(source),
            ),
            Err(source) => self.report(
                "Couldn't sign out everywhere.",
                &AccountRequestError::SignOutEverywhere(source),
            ),
        }
    }
}

#[derive(Debug)]
enum BrowserFailure {
    Problem(Problem),
    Receive(ReceiveError),
}

impl BrowserFailure {
    fn message(&self, provider: Provider) -> String {
        match self {
            Self::Problem(problem) => problem.message(provider),
            Self::Receive(error) => {
                tracing::warn!(
                    provider = provider.name(),
                    "browser sign-in failed: {}",
                    ::diagnostics::chain(error)
                );
                match error {
                    ReceiveError::TimedOut { .. } => Problem::Unfinished.message(provider),
                    ReceiveError::Listen(_) | ReceiveError::Accept(_) => format!(
                        "Couldn't receive the {} sign-in from your browser. Press the button to try again.",
                        provider.name()
                    ),
                }
            }
        }
    }
}

fn is_session_invalid(error: &NetworkError) -> bool {
    matches!(error, NetworkError::Core { code, .. } if matches!(code.as_str(), "unauthorized" | "gone"))
}
fn plain_failure(error: &NetworkError) -> String {
    match error.core_code() {
        Some("bad_request") => "Core could not use that request. Check the email address.",
        Some("unauthorized") => "That sign-in link is invalid or already used.",
        Some("forbidden") => "The email link is not approved yet, or the second factor is missing.",
        Some("gone") => "That sign-in request expired. Request a new link.",
        Some("rate_limited") => "Too many attempts. Wait a while and try again.",
        Some(_) => "Core could not do that right now. Try again in a bit.",
        None => "Could not reach Core. Check the connection and try again.",
    }
    .to_owned()
}

impl std::fmt::Debug for AccountPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::SignedOut => "SignedOut",
            Self::Restoring => "Restoring",
            Self::Requesting => "Requesting",
            Self::Browser { .. } => "Browser",
            Self::Pending { .. } => "Pending",
            Self::EmailApproved { .. } => "EmailApproved",
            Self::Finalizing { .. } => "Finalizing",
            Self::Authenticated { .. } => "Authenticated",
            Self::Error { .. } => "Error",
        };
        f.debug_struct(name).finish_non_exhaustive()
    }
}

impl std::fmt::Debug for Account {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Account")
            .field("phase", &self.phase)
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}
