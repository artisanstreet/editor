//! Which of the account's Forges are live, and how each is reached: this
//! machine's enrolled Forge directly over mutual TLS, every other one
//! through the Relay. Signing out drops them all; files stay on disk.

use anyhow::{Context as _, Result};
use gpui::{Context, Task};

use super::{EntryGate, live_machine};
use crate::account::{AccountMachine, LocalForge};
use crate::hosts::pair_this_machine;
use crate::services::open_relay;
use crate::{Core, CoreEvent, ForgeClient, HostConfig, HostId, HostKind, Screen, SettingsPage};

pub enum Dial {
    Opening(#[expect(dead_code, reason = "held so that dropping the dial cancels it")] Task<()>),
    /// Retried when Core next confirms the session, or on an explicit connect.
    Failed,
}

impl Core {
    #[must_use]
    pub fn entry_gate(&self, cx: &gpui::App) -> EntryGate {
        self.account.read(cx).entry_gate()
    }

    /// Opens a route to one of the account's Forges.
    pub fn connect_machine(&mut self, machine_id: &str, cx: &mut Context<Self>) {
        let id = HostId(machine_id.to_owned());
        self.dials.remove(&id);
        self.dial(&id, cx);
    }

    /// Records that setup finished on `machine_id`, whose route must be live.
    pub fn complete_setup(&mut self, machine_id: String, cx: &mut Context<Self>) -> Result<()> {
        let live = self
            .hosts
            .get(&HostId(machine_id.clone()))
            .is_some_and(|host| host.read(cx).is_live());
        anyhow::ensure!(live, "connect the Forge first, then finish setup");
        self.account.update(cx, |account, cx| {
            account.complete_onboarding(machine_id, cx)
        })
    }

    fn dial(&mut self, id: &HostId, cx: &Context<Self>) {
        if self.hosts.contains_key(id) || self.dials.contains_key(id) {
            return;
        }
        let account = self.account.read(cx);
        let Some(profile) = account.profile() else {
            return;
        };
        let Some(machine) = live_machine(profile, &id.0).cloned() else {
            return;
        };
        let direct = matches!(
            account.local_forge(),
            LocalForge::Enrolled { machine_id } if machine_id == machine.id
        );
        let account_id = profile.account_id.clone();
        let task = if direct {
            self.dial_direct(machine, cx)
        } else {
            self.dial_relay(account_id, machine, cx)
        };
        self.dials.insert(id.clone(), Dial::Opening(task));
    }

    /// Pairs with the local Forge through `ae pair --json` the first time;
    /// afterwards the saved pairing in `hosts.json` is enough, even offline.
    fn dial_direct(&self, machine: AccountMachine, cx: &Context<Self>) -> Task<()> {
        let id = HostId(machine.id.clone());
        let saved = self
            .hosts_file
            .hosts
            .iter()
            .find(|config| config.id == id)
            .cloned();
        let identity = self.identity.clone();
        let ae = self.account.read(cx).services().ae.clone();
        let device_name = self.device_name.clone();
        cx.spawn(async move |this, cx| {
            let opened = async {
                let identity = identity.context("this device has no identity to pair with")?;
                let config = match saved {
                    Some(config) => config,
                    None => {
                        pair_this_machine(&ae, &identity, &device_name, id.clone(), machine.label)
                            .await?
                    }
                };
                let client = ForgeClient::new(&config.address, &config.fingerprint, &identity)?;
                anyhow::Ok((config, client))
            }
            .await
            .context("connect to this machine's Forge");
            // Err: Core is gone.
            this.update(cx, |core, cx| core.opened(&id, opened, cx))
                .ok();
        })
    }

    fn dial_relay(
        &self,
        account_id: String,
        machine: AccountMachine,
        cx: &Context<Self>,
    ) -> Task<()> {
        let account = self.account.read(cx);
        let route = account.services().relay.clone();
        let (http, session) = (account.http().clone(), account.session());
        let id = HostId(machine.id.clone());
        let config = HostConfig {
            id: id.clone(),
            name: machine.label.clone(),
            address: String::new(),
            fingerprint: String::new(),
            kind: HostKind::Account {
                account: account_id.clone(),
                machine: machine.id.clone(),
            },
        };
        cx.spawn(async move |this, cx| {
            let label = machine.label.clone();
            let opened = async {
                let session = session.context("sign in first")?;
                open_relay(route, http, session, account_id, machine).await
            }
            .await
            .map(|client| (config, client))
            .with_context(|| format!("connect to {label} through the Relay"));
            // Err: Core is gone.
            this.update(cx, |core, cx| core.opened(&id, opened, cx))
                .ok();
        })
    }

    fn opened(
        &mut self,
        id: &HostId,
        opened: Result<(HostConfig, ForgeClient)>,
        cx: &mut Context<Self>,
    ) {
        let (config, client) = match opened {
            Ok(opened) => opened,
            Err(error) => {
                self.dials.insert(id.clone(), Dial::Failed);
                self.account
                    .update(cx, |account, cx| account.report(&error, cx));
                return;
            }
        };
        self.dials.remove(id);
        if matches!(config.kind, HostKind::Account { .. }) {
            self.relay_configs.insert(id.clone(), config.clone());
        } else if !self.hosts_file.hosts.contains(&config) {
            let mut file = self.hosts_file.clone();
            file.hosts.retain(|known| known.id != config.id);
            file.hosts.push(config.clone());
            match self.files.save_hosts(&file) {
                Ok(()) => self.hosts_file = file,
                Err(error) => {
                    let error = error.context("save the pairing with this machine's Forge");
                    self.account
                        .update(cx, |account, cx| account.report(&error, cx));
                }
            }
        }
        self.connect(&config, client, cx);
        cx.emit(CoreEvent::HostsChanged);
        cx.notify();
    }

    /// Drops host entities only: `hosts.json` and the Forges keep their data.
    fn drop_hosts(&mut self, ids: &[HostId], cx: &mut Context<Self>) {
        if ids.is_empty() {
            return;
        }
        for id in ids {
            if let Some(host) = self.hosts.remove(id) {
                host.read(cx).client.close_relay();
            }
            self.relay_configs.remove(id);
            self.dials.remove(id);
        }
        if self.screen.host().is_some_and(|host| ids.contains(host)) {
            self.screen = Screen::Home;
            cx.emit(CoreEvent::ScreenChanged);
        }
        self.back
            .retain(|screen| screen.host().is_none_or(|host| !ids.contains(host)));
        self.pending_screen = self
            .pending_screen
            .take()
            .filter(|screen| screen.host().is_none_or(|host| !ids.contains(host)));
        cx.emit(CoreEvent::HostsChanged);
    }

    /// Brings the live hosts in line with the account after any change.
    pub(crate) fn reconcile(&mut self, cx: &mut Context<Self>) {
        let account = self.account.read(cx);
        let gate = account.entry_gate();
        self.gate = gate.clone();
        let Some(profile) = account.profile().cloned() else {
            let ids: Vec<HostId> = self.hosts.keys().cloned().collect();
            self.dials.clear();
            self.drop_hosts(&ids, cx);
            cx.emit(CoreEvent::AccessChanged);
            cx.notify();
            return;
        };
        let reading = account.reading_local_forge();
        let local = match account.local_forge() {
            LocalForge::Enrolled { machine_id } => Some(machine_id),
            LocalForge::Unknown
            | LocalForge::Unenrolled
            | LocalForge::Pending
            | LocalForge::Claimed { .. }
            | LocalForge::Revoked { .. }
            | LocalForge::OtherAccount => None,
        };
        let onboarded = account.onboarded_machine().map(str::to_owned);
        let removed = |id: &HostId| {
            profile
                .machines
                .iter()
                .any(|machine| machine.id == id.0 && machine.revoked_at.is_some())
        };
        let stale: Vec<HostId> = self
            .hosts
            .keys()
            .filter(|id| {
                removed(id)
                    || (self.relay_configs.contains_key(*id)
                        && live_machine(&profile, &id.0).is_none())
            })
            .cloned()
            .collect();
        self.drop_hosts(&stale, cx);
        self.dials
            .retain(|id, _| live_machine(&profile, &id.0).is_some());
        // Until `ae` answers, the local Forge might be the onboarded one.
        if !reading {
            if let Some(machine_id) = local {
                self.dial(&HostId(machine_id), cx);
            }
            if let (EntryGate::Ready { .. }, Some(machine_id)) = (&gate, onboarded) {
                self.dial(&HostId(machine_id), cx);
            }
        }
        if matches!(gate, EntryGate::Ready { .. })
            && let Some(screen) = self.pending_screen.take()
        {
            self.navigate(screen, cx);
        }
        cx.emit(CoreEvent::AccessChanged);
        cx.notify();
    }

    /// Only Home and the Account page load before the gate is `Ready`.
    pub(crate) const fn screen_allowed_while_locked(screen: &Screen) -> bool {
        matches!(
            screen,
            Screen::Home | Screen::Settings(SettingsPage::Account)
        )
    }
}
