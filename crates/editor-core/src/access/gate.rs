//! Account-only gate enforcement for [`Core`]: entry, inventory,
//! setup completion, and reconciliation. Semantics unchanged from
//! the former `lib.rs` implementation.

use super::EntryGate;
use crate::{
    Account, AccountPhase, Core, CoreEvent, ForgeClient, HostConfig, HostData, HostId, HostKind,
    Screen, SettingsPage,
};
use anyhow::{Context as _, Result};
use gpui::{Context, Entity};

impl Core {
    /// Account-only entry gate for `EntryRoot`. Cached per origin+account
    /// transition (no per-frame disk IO); `Restoring` holds the workspace
    /// unmounted with no host connects; only `Ready` exposes content.
    #[must_use]
    pub fn entry_gate(&self, cx: &gpui::App) -> EntryGate {
        let account = self.account.read(cx);
        match account.phase() {
            AccountPhase::Restoring => EntryGate::Restoring,
            AccountPhase::Authenticated { profile } => {
                let account_id = profile.account_id.clone();
                let email = profile.email.clone();
                let onboarded = self.onboarding_cache.as_ref().is_some_and(|record| {
                    Some(&record.account_id) == Some(&account_id)
                        && Some(&record.base_url) == Some(&account.base_url().to_owned())
                        && profile.machines.iter().any(|machine| {
                            machine.id == record.machine_id && machine.revoked_at.is_none()
                        })
                }) && self.onboarding_scope.as_ref().is_some_and(|(base, aid)| {
                    base == account.base_url() && aid == &account_id
                });
                if onboarded {
                    EntryGate::Ready { account_id }
                } else {
                    EntryGate::NeedsSetup { account_id, email }
                }
            }
            AccountPhase::SignedOut
            | AccountPhase::Requesting
            | AccountPhase::Pending { .. }
            | AccountPhase::EmailApproved { .. }
            | AccountPhase::Finalizing
            | AccountPhase::Error { .. } => EntryGate::SignedOut,
        }
    }

    /// Guarded host access: `Some` only when the gate is `Ready` and the
    /// host is connected. Setup flows use private internals; public UI must
    /// go through here so `NeedsSetup`/locked states never leak content.
    #[must_use]
    pub fn require_host(
        &self,
        id: &HostId,
        cx: &gpui::App,
    ) -> Option<gpui::Entity<HostData>> {
        if !matches!(self.entry_gate(cx), EntryGate::Ready { .. }) {
            return None;
        }
        self.hosts.get(id).cloned()
    }

    /// Whether `machine_id` has a live account route under the current
    /// session. Used at `complete_setup` time; metadata alone is not enough.
    #[must_use]
    pub fn connected_account_host(&self, machine_id: &str, cx: &gpui::App) -> bool {
        let account = self.account.read(cx);
        let Some(profile) = account.profile() else {
            return false;
        };
        let id = HostId(machine_id.to_owned());
        self.hosts.contains_key(&id)
            && self.account_hosts.get(&id).is_some_and(|owner| *owner == profile.account_id)
            && self.hosts.get(&id).is_some_and(|host| {
                let host = host.read(cx);
                host.client.is_relay() && host.is_live()
            })
    }

    /// Adds an in-memory host entry for a connected Relay Forge. Validates
    /// the current account/session generation at insertion: stale
    /// `connect_machine` callbacks after sign-out/switch are dropped, not
    /// just unsubscribed. Allowed in `Ready` and `NeedsSetup` (Relay
    /// onboarding); rejected while locked.
    pub fn connect_with_client(
        &mut self,
        config: &HostConfig,
        client: ForgeClient,
        cx: &mut Context<Self>,
    ) {
        let (phase_ok, owner_ok) = self.account.read_with(cx, |account, _| {
            let phase_ok = matches!(account.phase(), AccountPhase::Authenticated { .. });
            let owner_ok = match &config.kind {
                HostKind::Account { account: owner, machine } => account
                    .profile()
                    .is_some_and(|profile| &profile.account_id == owner)
                    && account.can_complete_setup(machine)
                    && account.session_token().is_some(),
                HostKind::Local | HostKind::Wsl { .. } | HostKind::Remote => false,
            };
            (phase_ok, owner_ok)
        });
        if !phase_ok || !owner_ok {
            return;
        }
        if let HostKind::Account { account, .. } = &config.kind {
            self.account_hosts.insert(config.id.clone(), account.clone());
            // Ephemeral inventory so `hosts()`/`active_host()` expose the
            // Relay route once `Ready`. Never persisted to `hosts.json`.
            self.account_configs.insert(config.id.clone(), config.clone());
        }
        self.hosts.remove(&config.id);
        self.connect(config, client, cx);
        cx.emit(CoreEvent::HostsChanged);
        cx.notify();
    }

    /// Completes onboarding against a real connected account-owned Forge:
    /// the live profile must list a non-revoked machine under the current
    /// account and origin, and the exact mapped Relay route must be live.
    /// Persists the per-origin+account record, then applies deferred
    /// navigation. Setup internals may read private host state while public
    /// `hosts()` stays hidden until `Ready`.
    pub fn complete_setup(
        &mut self,
        machine_id: String,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        let (profile, base_url) = self.account.read_with(cx, |account, _| {
            (
                account.profile().cloned(),
                account.base_url().to_owned(),
            )
        });
        let Some(profile) = profile else {
            anyhow::bail!("sign in first");
        };
        let machine = profile
            .machines
            .iter()
            .find(|machine| machine.id == machine_id)
            .context("that Forge is no longer on the account")?;
        if machine.revoked_at.is_some() {
            anyhow::bail!("that Forge was removed from the account");
        }
        let account_id = profile.account_id.clone();
        // Live connection proof: the exact mapped Relay route must be live
        // (disconnected placeholders don't count). Direct adoption has no
        // exact local-route proof in the current API (a same-device live
        // candidate and matching key do not tie the candidate TLS identity
        // to the enrolled machine, especially across WSL/data dirs), so it
        // is rejected honestly instead of persisting an assumed binding.
        // Same Forge Hub data is preserved: onboarding goes through
        // verified Relay; direct pairings stay on disk awaiting a proven
        // adoption flow.
        let relay_live = self.hosts.contains_key(&HostId(machine_id.clone()))
            && self
                .account_hosts
                .get(&HostId(machine_id.clone()))
                .is_some_and(|owner| *owner == account_id)
            && self
                .hosts
                .get(&HostId(machine_id.clone()))
                .is_some_and(|host| {
                    host.read_with(cx, |host, _| host.client.is_relay() && host.is_live())
                });
        if !relay_live {
            anyhow::bail!(
                "connect the Forge through Relay first, then finish setup (direct adoption is not available yet; your direct pairings stay on disk)"
            );
        }
        let completed_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs().to_string())
            .unwrap_or_default();
        let record = crate::access::OnboardingRecord {
            account_id: account_id.clone(),
            base_url: base_url.clone(),
            machine_id,
            completed_at,
        };
        crate::access::save_onboarding(&self.files.dir, &record)?;
        self.onboarding_cache = Some(record);
        let scope = (base_url, account_id);
        self.onboarding_scope = Some(scope.clone());
        self.gate_ready = Some(scope);
        // The Relay route proving setup is already live; no legacy
        // direct dial here. Unknown hosts stay disconnected.
        cx.emit(CoreEvent::AccessChanged);
        cx.notify();
        let pending = self.pending_screen.take();
        if let Some(screen) = pending {
            self.navigate(screen, cx);
        }
        Ok(())
    }

    /// Restarts with a cached Relay onboarding must not dead-end on the
    /// legacy pair UI: `Ready` + empty hosts auto-reconnects the recorded
    /// machine through the account after borrow release. Guarded to one
    /// attempt per Ready scope; manual Connect stays available afterwards.
    /// Unknown legacy direct hosts are never dialed here.
    fn maybe_autoconnect_relay(
        &mut self,
        account: &Entity<Account>,
        machine_id: String,
        cx: &mut Context<Self>,
    ) {
        if self.relay_autoconnected.as_deref() == Some(machine_id.as_str()) {
            return;
        }
        if self.hosts.contains_key(&HostId(machine_id.clone())) {
            return;
        }
        self.relay_autoconnected = Some(machine_id.clone());
        let core = cx.entity();
        account.update(cx, |account, cx| {
            account.connect_machine(machine_id, &core, cx);
        });
    }

    /// Drops host entities without touching persisted pairings or Forge disk
    /// data. Closes Relay routes first so view-held clones stop at once.
    fn drop_hosts(&mut self, ids: &[HostId], cx: &mut Context<Self>) {
        for id in ids {
            if let Some(host) = self.hosts.get(id) {
                host.read_with(cx, |host, _| host.client.close_relay());
            }
            self.hosts.remove(id);
            self.account_hosts.remove(id);
            self.account_configs.remove(id);
        }
        if self.screen.host().is_some_and(|host| ids.contains(host)) {
            self.screen = Screen::Home;
            cx.emit(CoreEvent::ScreenChanged);
        }
        self.back
            .retain(|screen| screen.host().is_none_or(|host| !ids.contains(host)));
        if self.hosts_file.last.as_ref().is_some_and(|last| ids.contains(last)) {
            let mut file = self.hosts_file.clone();
            file.last = None;
            if self.files.save_hosts(&file).is_ok() {
                self.hosts_file = file;
            }
        }
        // Deferred intent pointing at a dropped host is stale.
        if self
            .pending_screen
            .as_ref()
            .and_then(|screen| screen.host())
            .is_some_and(|host| ids.contains(host))
        {
            self.pending_screen = None;
        }
    }

    /// Locks all host access: closes every route, drops every entity,
    /// navigates home, prunes host back-stack. Retains disk data
    /// (`hosts.json`, Forge DBs); backend runs on Forges keep running.
    fn lock_all_hosts(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<HostId> = self.hosts.keys().cloned().collect();
        self.drop_hosts(&ids, cx);
        self.pending_screen = self.pending_screen.take().filter(|screen| screen.host().is_none());
        cx.emit(CoreEvent::HostsChanged);
        cx.notify();
    }

    /// Account transitions drive the gate: cached onboarding loads once per
    /// origin+account, hosts lock on sign-out, deferred hosts connect on
    /// `Ready`. Stale previous-account back-stack is pruned; deferred
    /// deeplinks survive `Restoring` but never load guarded content locked.
    pub(crate) fn reconcile_access(&mut self, account: &Entity<Account>, cx: &mut Context<Self>) {
        let (phase, profile, token, base_url) = account.read_with(cx, |account, _| {
            (
                account.phase().clone(),
                account.profile().cloned(),
                account.session_token().map(|token| token.as_str().to_owned()),
                account.base_url().to_owned(),
            )
        });
        let rotated = self.account_session.as_deref() != token.as_deref();
        self.account_session = token;
        match (&phase, profile) {
            (AccountPhase::Restoring, _) => {
                self.onboarding_cache = None;
                self.onboarding_scope = None;
                self.relay_autoconnected = None;
                self.gate_ready = None;
                self.lock_all_hosts(cx);
                cx.emit(CoreEvent::AccessChanged);
                cx.notify();
            }
            (AccountPhase::Authenticated { .. }, Some(profile)) => {
                let account_id = profile.account_id.clone();
                let scope = (base_url.clone(), account_id.clone());
                if self.onboarding_scope.as_ref() != Some(&scope) {
                    self.onboarding_cache =
                        crate::access::load_onboarding(&self.files.dir, &base_url, &account_id);
                    self.onboarding_scope = Some(scope.clone());
                }
                let ready = self.onboarding_cache.as_ref().is_some_and(|record| {
                    record.account_id == account_id
                        && record.base_url == base_url
                        && profile.machines.iter().any(|machine| {
                            machine.id == record.machine_id && machine.revoked_at.is_none()
                        })
                });
                if ready {
                    // Drop rotated or foreign account routes, keep current.
                    let stale: Vec<HostId> = self
                        .account_hosts
                        .iter()
                        .filter(|&(_, owner)| rotated || *owner != account_id)
                        .map(|(id, _)| id.clone())
                        .collect();
                    if !stale.is_empty() {
                        self.drop_hosts(&stale, cx);
                        self.relay_autoconnected = None;
                    }
                    // Restart path: cached onboarding with empty hosts
                    // auto-reconnects the recorded machine through the
                    // account (after borrow release). Ready Home shows the
                    // account machine inventory, never the legacy pair UI.
                    if let Some(record) = self.onboarding_cache.clone()
                        && record.account_id == account_id
                        && record.base_url == scope.0
                    {
                        let machine = record.machine_id;
                        if !self.hosts.contains_key(&HostId(machine.clone())) {
                            self.maybe_autoconnect_relay(account, machine, cx);
                        }
                    }
                    self.gate_ready = Some(scope);
                    cx.emit(CoreEvent::AccessChanged);
                    cx.notify();
                    let pending = self.pending_screen.take();
                    if let Some(screen) = pending {
                        self.navigate(screen, cx);
                    }
                } else {
                    // Setup: keep only current account routes for onboarding,
                    // drop every direct host; public inventory stays hidden.
                    self.gate_ready = None;
                    self.relay_autoconnected = None;
                    let mut stale: Vec<HostId> = self
                        .hosts
                        .keys()
                        .filter(|id| !self.account_hosts.contains_key(*id))
                        .cloned()
                        .collect();
                    stale.extend(
                        self.account_hosts
                            .iter()
                            .filter(|&(_, owner)| rotated || *owner != account_id)
                            .map(|(id, _)| id.clone()),
                    );
                    if !stale.is_empty() {
                        self.drop_hosts(&stale, cx);
                    }
                    cx.emit(CoreEvent::AccessChanged);
                    cx.notify();
                }
            }
            _ => {
                // SignedOut and all in-flight sign-in steps: locked.
                self.onboarding_cache = None;
                self.onboarding_scope = None;
                self.relay_autoconnected = None;
                self.gate_ready = None;
                // A replaced token retires channels even under the same id;
                // here there is no valid session at all, so lock everything.
                self.lock_all_hosts(cx);
                cx.emit(CoreEvent::AccessChanged);
                cx.notify();
            }
        }
    }


    /// Whether `screen` may load while locked. Only `Home` and the Account
    /// settings page are unguarded; everything else defers until `Ready`
    /// and never loads guarded content while locked.
    pub(crate) const fn screen_allowed_while_locked(screen: &Screen) -> bool {
        matches!(screen, Screen::Home | Screen::Settings(SettingsPage::Account))
    }


}