mod enrollment;

use crate::core::{
    access::{self, EntryGate, OnboardingRecord},
    delivery::{AdmissionOrder, Delivery},
    hosts::{EditorFiles, read_json, write_json},
    runtime::runtime,
    services::Services,
};
use anyhow::{Context as _, Result};
use artisan_account::network::{AccountProfile, CoreSessionClient, NetworkError, SessionToken};
use enrollment::AeBinding;
pub use enrollment::LocalForge;
use serde::{Deserialize, Serialize};
use std::{num::NonZeroUsize, time::Duration};
use tokio_util::task::AbortOnDropHandle;

pub type AccountMachine = artisan_account::network::MachineEntry;

#[derive(Clone, PartialEq, Eq)]
pub enum AccountPhase {
    SignedOut,
    Restoring,
    Requesting,
    Pending {
        request_id: String,
        request_secret: String,
        email: String,
        expires_at: String,
    },
    EmailApproved {
        request_id: String,
        request_secret: String,
        email: String,
        totp_required: bool,
    },
    Finalizing,
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
    Synced(
        u64,
        Result<AccountProfile, NetworkError>,
        tokio::sync::oneshot::Sender<()>,
    ),
    Binding(u64, Result<AeBinding>, bool),
    Request(u64, Result<(), NetworkError>, bool),
}

#[cfg(feature = "test-support")]
pub struct TestAccountTransitions {
    sender: crate::core::delivery::DeliverySender<Message>,
    generation: u64,
}
#[cfg(feature = "test-support")]
impl TestAccountTransitions {
    pub async fn profile(&self, profile: AccountProfile) {
        let (applied, _) = tokio::sync::oneshot::channel();
        assert!(
            self.sender
                .send(Message::Synced(self.generation, Ok(profile), applied))
                .await
                .is_ok()
        );
    }
    pub async fn transport_failure(&self) {
        drop(self.transport_failure_ack().await);
    }
    pub async fn transport_failure_ack(&self) -> tokio::sync::oneshot::Receiver<()> {
        let (applied, acknowledged) = tokio::sync::oneshot::channel();
        assert!(
            self.sender
                .send(Message::Synced(
                    self.generation,
                    Err(NetworkError::Transport),
                    applied
                ))
                .await
                .is_ok()
        );
        acknowledged
    }
}

pub(crate) struct AccountEvents {
    applied: Option<tokio::sync::oneshot::Sender<()>>,
}
impl Drop for AccountEvents {
    fn drop(&mut self) {
        if let Some(applied) = self.applied.take() {
            let _ = applied.send(());
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
    last_error: Option<String>,
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

impl Account {
    pub(crate) fn new(
        files: EditorFiles,
        services: Services,
        wake: aqua::mailbox::Sender<()>,
        order: AdmissionOrder,
        effects: crate::core::store::Effects,
    ) -> Self {
        let (stored, unreadable) = match read_json::<AccountFile>(&files.dir.join("account.json")) {
            Ok(stored) => (stored, None),
            Err(error) => (AccountFile::default(), Some(error)),
        };
        let base_url = services
            .core_url
            .clone()
            .or(stored.base_url)
            .or_else(|| std::env::var("AST_CORE_BASE_URL").ok())
            .filter(|url| !url.trim().is_empty())
            .unwrap_or_else(|| "https://api.artisan.st".to_owned());
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
            delivery: Delivery::with_order(NonZeroUsize::new(64).unwrap(), wake, order),
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

    #[cfg(feature = "test-support")]
    pub fn test_sync_sender(&mut self) -> TestAccountTransitions {
        self.sync = None;
        self.sync_generation = self.sync_generation.wrapping_add(1);
        TestAccountTransitions {
            sender: self.delivery.sender(),
            generation: self.sync_generation,
        }
    }

    pub fn phase(&self) -> &AccountPhase {
        &self.phase
    }
    pub fn profile(&self) -> Option<&AccountProfile> {
        match &self.phase {
            AccountPhase::Authenticated { profile, .. } => Some(profile),
            _ => None,
        }
    }
    pub fn machines(&self) -> Vec<AccountMachine> {
        self.profile()
            .map(|profile| access::live_machines(profile).cloned().collect())
            .unwrap_or_default()
    }
    pub fn entry_gate(&self) -> EntryGate {
        access::entry_gate(&self.phase, self.onboarding.as_ref())
    }
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }
    pub fn base_url(&self) -> &str {
        &self.base_url
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
    fn changed(&mut self) {
        self.events.account(AccountEvent::Changed);
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
        let Some(id) = self.profile().map(|p| p.account_id.clone()) else {
            self.onboarding = None;
            return;
        };
        match access::load_onboarding(&self.files.dir, &self.base_url, &id) {
            Ok(record) => self.onboarding = record,
            Err(error) => {
                self.onboarding = None;
                self.last_error = Some(format!("{error:#}"));
            }
        }
    }
    pub(crate) fn complete_onboarding(&mut self, machine_id: String) -> Result<()> {
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
        self.changed();
        Ok(())
    }
    pub(crate) fn report(&mut self, error: &anyhow::Error) {
        tracing::warn!("{error:#}");
        self.last_error = Some(format!("{error:#}"));
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
            self.last_error = Some(format!("{error:#}"));
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
                let done = !matches!(
                    &result,
                    Err(NetworkError::Transport) | Ok(SignInStatus::Waiting)
                );
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

    pub fn finish_sign_in(&mut self, totp_code: Option<String>, recovery_code: Option<String>) {
        let AccountPhase::EmailApproved {
            request_id,
            request_secret,
            ..
        } = &self.phase
        else {
            return;
        };
        let (id, secret) = (request_id.clone(), request_secret.clone());
        self.sign_generation += 1;
        let generation = self.sign_generation;
        let http = self.http.clone();
        let sender = self.delivery.sender();
        self.phase = AccountPhase::Finalizing;
        self.changed();
        self.sign_in = Some(AbortOnDropHandle::new(runtime().spawn(async move {
            let result = http
                .magic_finalize(
                    &id,
                    &secret,
                    totp_code.as_deref().filter(|code| !code.trim().is_empty()),
                    recovery_code
                        .as_deref()
                        .filter(|code| !code.trim().is_empty()),
                )
                .await
                .map(|(_, token)| token);
            let _ = sender.send(Message::Finalized(generation, result)).await;
        })));
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
                    tracing::warn!("couldn't revoke the session at Core: {error:#}");
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
            let _ = sender
                .send(Message::Request(generation, result, remove))
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
            let _ = sender
                .send(Message::Binding(generation, result, false))
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
        let (ae, core, http, sender) = (
            self.services.ae.clone(),
            self.base_url.clone(),
            self.http.clone(),
            self.delivery.sender(),
        );
        self.last_error = None;
        self.forge = Some(AbortOnDropHandle::new(runtime().spawn(async move {
            let result = async {
                let mut args = vec!["account", "--core", &core, "enroll", "--json"];
                if replace {
                    args.push("--replace");
                }
                let enrollment: enrollment::Enrollment =
                    serde_json::from_str(&ae.run(&args).await?)
                        .context("read what `ae account enroll` printed")?;
                let code = artisan_account::normalise_code(&enrollment.code);
                http.claim_enrollment(&session, &code).await?;
                enrollment::read_status(&ae, &core).await
            }
            .await
            .context("enroll this machine's Forge");
            let _ = sender
                .send(Message::Binding(generation, result, false))
                .await;
        })));
    }
    pub fn claim_code(&mut self, code: String) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let code = artisan_account::normalise_code(&code);
        self.forge_generation += 1;
        let generation = self.forge_generation;
        let (ae, core, http, sender) = (
            self.services.ae.clone(),
            self.base_url.clone(),
            self.http.clone(),
            self.delivery.sender(),
        );
        self.last_error = None;
        self.forge = Some(AbortOnDropHandle::new(runtime().spawn(async move {
            let result = async {
                http.claim_enrollment(&session, &code).await?;
                enrollment::read_status(&ae, &core).await
            }
            .await
            .context("claim the Forge's code");
            let _ = sender
                .send(Message::Binding(generation, result, false))
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
                .await?;
                enrollment::read_status(&ae, &core).await
            }
            .await
            .context("confirm this machine's Forge");
            let _ = sender
                .send(Message::Binding(generation, result, true))
                .await;
        })));
    }
    #[cfg(feature = "test-support")]
    pub(crate) fn sign_in_for_tests(&mut self, profile: AccountProfile, machine_id: String) {
        self.sign_in = None;
        self.sync = None;
        self.forge = None;
        self.request = None;
        self.sign_generation = self.sign_generation.wrapping_add(1);
        self.sync_generation = self.sync_generation.wrapping_add(1);
        self.forge_generation = self.forge_generation.wrapping_add(1);
        self.request_generation = self.request_generation.wrapping_add(1);
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
        self.changed();
    }

    pub(crate) fn peek_order(&mut self) -> Option<u64> {
        self.delivery.peek_order()
    }
    pub(crate) fn drain(&mut self) -> AccountEvents {
        let mut acknowledgement = None;
        for message in self.delivery.drain_one().into_iter() {
            match message {
                Message::Requested(generation, email, result, applied)
                    if generation == self.sign_generation =>
                {
                    acknowledgement = Some(applied);
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
                Message::Status(generation, id, secret, result, applied)
                    if generation == self.sign_generation =>
                {
                    acknowledgement = Some(applied);
                    match result {
                        Err(NetworkError::Transport) => {}
                        Err(error) => self.fail_sign_in(&error),
                        Ok(SignInStatus::Waiting) => {}
                        Ok(SignInStatus::Approved {
                            email,
                            totp_required,
                        }) => {
                            self.phase = AccountPhase::EmailApproved {
                                request_id: id,
                                request_secret: secret,
                                email,
                                totp_required,
                            };
                            self.changed();
                        }
                        Ok(SignInStatus::Expired) => {
                            self.phase = AccountPhase::Error {
                                message: "That sign-in request expired. Request a new link."
                                    .to_owned(),
                            };
                            self.changed();
                        }
                    }
                }
                Message::Finalized(generation, result) if generation == self.sign_generation => {
                    match result {
                        Ok(token) => {
                            self.session = Some(token);
                            self.phase = AccountPhase::Restoring;
                            if let Err(error) = self.persist() {
                                self.last_error = Some(format!("{error:#}"));
                            }
                            self.changed();
                            self.refresh();
                        }
                        Err(error) => self.fail_sign_in(&error),
                    }
                }
                Message::Synced(generation, result, applied)
                    if generation == self.sync_generation =>
                {
                    acknowledgement = Some(applied);
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
                                self.last_error = Some(format!("{error:#}"));
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
                Message::Binding(generation, result, refresh)
                    if generation == self.forge_generation =>
                {
                    self.forge = None;
                    match result {
                        Ok(binding) => {
                            self.binding = Some(binding);
                            self.changed();
                            if refresh {
                                self.refresh();
                            }
                        }
                        Err(error) => self.report(&error),
                    }
                }
                Message::Request(generation, result, remove)
                    if generation == self.request_generation =>
                {
                    self.request = None;
                    match result {
                        Ok(()) if remove => self.refresh(),
                        Ok(()) => self.clear_session(),
                        Err(error) if is_session_invalid(&error) => self.clear_session(),
                        Err(error) => self.report(&anyhow::Error::new(error).context(if remove {
                            "remove the Forge"
                        } else {
                            "sign out everywhere"
                        })),
                    }
                }
                _ => {}
            }
        }
        AccountEvents {
            applied: acknowledgement,
        }
    }
}

pub fn is_session_invalid(error: &NetworkError) -> bool {
    matches!(error, NetworkError::Core { code, .. } if matches!(code.as_str(), "unauthorized" | "gone"))
}
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
