use crate::core::{
    ForgeClient,
    access::{self, EntryGate},
    account::{Account, AccountEvent, LocalForge},
    delivery::{AdmissionOrder, Delivery},
    host::{HostData, HostEvent, HostInstance},
    hosts::{self, EditorFiles, EditorSettings, HostConfig, HostId, HostKind, HostsFile},
    mcp_sign_in::{McpEvent, McpSignIn, McpSignIns},
    runtime::runtime,
    screen::{Screen, SettingsPage},
    services::{self, Services},
};
use anyhow::{Context as _, Result};
use artisan_api::tls::Identity;
use std::{
    collections::{HashMap, VecDeque},
    num::NonZeroUsize,
};
use tokio_util::task::AbortOnDropHandle;

#[derive(Clone, Debug)]
pub enum CoreEvent {
    ScreenChanged,
    HostsChanged,
    SettingsChanged,
    AccessChanged,
    McpSignInChanged,
    OpenUrl(String),
    Account(AccountEvent),
    Host { host: HostId, event: HostEvent },
}

enum Dial {
    Opening { _task: AbortOnDropHandle<()> },
    Failed,
}
struct Opened {
    id: HostId,
    generation: u64,
    result: Result<(HostConfig, ForgeClient)>,
}

pub(crate) enum Effect {
    Core(CoreEvent),
    Account(AccountEvent),
    Mcp(McpEvent),
    Host(HostId, HostInstance, HostEvent),
}

#[derive(Clone)]
pub(crate) struct Effects {
    queue: std::rc::Rc<std::cell::RefCell<VecDeque<Effect>>>,
    wake: aqua::mailbox::Sender<()>,
    draining: std::rc::Rc<std::cell::Cell<bool>>,
}
impl Effects {
    fn new(wake: aqua::mailbox::Sender<()>, draining: std::rc::Rc<std::cell::Cell<bool>>) -> Self {
        Self {
            queue: Default::default(),
            wake,
            draining,
        }
    }
    fn enqueue(&self, effect: Effect) {
        self.queue.borrow_mut().push_back(effect);
        if !self.draining.get() {
            let _ = self.wake.send(());
        }
    }
    fn push(&self, event: CoreEvent) {
        self.enqueue(Effect::Core(event));
    }
    pub(crate) fn account(&self, event: AccountEvent) {
        self.enqueue(Effect::Account(event));
    }
    pub(crate) fn mcp(&self, event: McpEvent) {
        self.enqueue(Effect::Mcp(event));
    }
    pub(crate) fn host(&self, id: &HostId, instance: HostInstance, event: HostEvent) {
        self.enqueue(Effect::Host(id.clone(), instance, event));
    }
    fn pop(&self) -> Option<Effect> {
        self.queue.borrow_mut().pop_front()
    }
    #[cfg(feature = "test-support")]
    pub(crate) fn clear_host(&self, id: &HostId, instance: HostInstance) {
        self.queue.borrow_mut().retain(|effect| !matches!(effect, Effect::Host(host, token, _) if host == id && *token == instance));
    }
}

pub struct Core {
    pub files: EditorFiles,
    pub identity: Option<Identity>,
    pub settings: EditorSettings,
    pub device_name: String,
    hosts: HashMap<HostId, HostData>,
    hosts_file: HostsFile,
    relay_configs: HashMap<HostId, HostConfig>,
    screen: Screen,
    back: Vec<Screen>,
    pending_screen: Option<Screen>,
    gate: EntryGate,
    account: Account,
    image_assets: Option<aqua::images::ImageAssets>,
    mcp_sign_ins: McpSignIns,
    wake: aqua::mailbox::Mailbox<()>,
    opened: Delivery<Opened>,
    admission: AdmissionOrder,
    dials: HashMap<HostId, Dial>,
    dial_generations: HashMap<HostId, u64>,
    events: Effects,
    draining: std::rc::Rc<std::cell::Cell<bool>>,
}

struct DrainGuard(std::rc::Rc<std::cell::Cell<bool>>);
impl Drop for DrainGuard {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

impl Core {
    pub fn new(files: EditorFiles) -> Result<Self> {
        Self::with_services(files, Services::installed())
    }
    pub fn with_services(files: EditorFiles, services: Services) -> Result<Self> {
        let identity = Some(files.identity()?);
        let hosts_file = files.hosts()?;
        Ok(Self::assemble(files, services, identity, hosts_file))
    }
    pub fn init(files: EditorFiles) -> (Self, Option<anyhow::Error>) {
        match Self::new(files.clone()) {
            Ok(core) => (core, None),
            Err(error) => (
                Self::assemble(files, Services::installed(), None, HostsFile::default()),
                Some(error),
            ),
        }
    }
    fn assemble(
        files: EditorFiles,
        services: Services,
        identity: Option<Identity>,
        hosts_file: HostsFile,
    ) -> Self {
        let settings = files.settings().unwrap_or_else(|error| {
            tracing::warn!("ignored unreadable settings: {error:#}");
            EditorSettings::new()
        });
        let wake = aqua::mailbox::Mailbox::new(NonZeroUsize::MIN);
        let admission = AdmissionOrder::default();
        let draining = std::rc::Rc::new(std::cell::Cell::new(false));
        let events = Effects::new(wake.sender(), draining.clone());
        let account = Account::new(
            files.clone(),
            services,
            wake.sender(),
            admission.clone(),
            events.clone(),
        );
        let gate = account.entry_gate();
        let mcp_sign_ins = McpSignIns::new(wake.sender(), admission.clone(), events.clone());
        let opened = Delivery::with_order(
            NonZeroUsize::new(64).unwrap(),
            wake.sender(),
            admission.clone(),
        );
        let mut core = Self {
            files,
            identity,
            settings,
            device_name: device_name(),
            hosts: HashMap::new(),
            hosts_file,
            relay_configs: HashMap::new(),
            screen: Screen::Home,
            back: Vec::new(),
            pending_screen: None,
            gate,
            account,
            image_assets: None,
            mcp_sign_ins,
            wake,
            opened,
            admission,
            dials: HashMap::new(),
            dial_generations: HashMap::new(),
            events,
            draining,
        };
        core.reconcile();
        core
    }
    #[cfg(feature = "test-support")]
    pub fn for_tests(files: EditorFiles) -> Result<Self> {
        use artisan_account::network::{AccountProfile, Jwk, MachineEntry};
        let services = Services::scripted(
            "http://127.0.0.1:1",
            std::sync::Arc::new(|_| anyhow::bail!("no `ae` in component tests")),
            None,
        );
        let mut core = Self::with_services(files, services)?;
        let machine = MachineEntry {
            id: "mch_test0000000000000000000000".to_owned(),
            label: "Test Forge".to_owned(),
            public_key: Jwk {
                kty: "OKP".to_owned(),
                crv: "Ed25519".to_owned(),
                x: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_owned(),
            },
            created_at: "2026-01-01T00:00:00.000Z".to_owned(),
            revoked_at: None,
            last_seen_at: None,
        };
        let profile = AccountProfile {
            account_id: "acct_test00000000000000000000".to_owned(),
            email: "test@example.invalid".to_owned(),
            display_name: "Test".to_owned(),
            totp_enrolled: false,
            machines: vec![machine.clone()],
            latest_config_version: None,
        };
        core.account.sign_in_for_tests(profile, machine.id);
        core.gate = core.account.entry_gate();
        let identity = core.identity.clone().context("test identity")?;
        for config in core.hosts_file.hosts.clone() {
            let client = ForgeClient::new(&config.address, &config.fingerprint, &identity)?;
            let mut host = HostData::new(
                config.id.clone(),
                config.name.clone(),
                client,
                core.wake.sender(),
                core.admission.clone(),
                core.events.clone(),
            );
            host.set_pairing(&config.kind, &config.address);
            if let Some(assets) = &core.image_assets {
                host.set_image_assets(assets.clone());
            }
            core.hosts.insert(config.id, host);
        }
        Ok(core)
    }

    #[cfg(feature = "test-support")]
    pub fn add_test_host(&mut self, config: &HostConfig, client: ForgeClient) {
        let mut file = self.hosts_file.clone();
        file.hosts.retain(|known| known.id != config.id);
        file.hosts.push(config.clone());
        file.last = Some(config.id.clone());
        if let Err(error) = self.files.save_hosts(&file) {
            tracing::warn!("test hosts.json: {error:#}");
        }
        self.hosts_file = file;
        let mut host = HostData::new(
            config.id.clone(),
            config.name.clone(),
            client,
            self.wake.sender(),
            self.admission.clone(),
            self.events.clone(),
        );
        host.set_pairing(&config.kind, &config.address);
        if let Some(assets) = &self.image_assets {
            host.set_image_assets(assets.clone());
        }
        self.hosts.insert(config.id.clone(), host);
        self.events.push(CoreEvent::HostsChanged);
    }

    pub fn set_image_assets(&mut self, assets: aqua::images::ImageAssets) {
        for host in self.hosts.values_mut() {
            host.set_image_assets(assets.clone());
        }
        self.image_assets = Some(assets);
    }
    pub fn wake_mailbox(&self) -> &aqua::mailbox::Mailbox<()> {
        &self.wake
    }

    #[cfg(feature = "test-support")]
    pub fn test_startup_pending(&mut self) -> bool {
        let admission = self.admission.clone();
        admission.inspect(|| {
            let admitted = self.account.peek_order().is_some()
                || self.mcp_sign_ins.peek_order().is_some()
                || self.opened.peek_order().is_some()
                || self
                    .hosts
                    .values_mut()
                    .any(|host| host.peek_order().is_some());
            let opening = self
                .dials
                .values()
                .any(|dial| matches!(dial, Dial::Opening { .. }));
            admitted || opening || !self.events.queue.borrow().is_empty()
        })
    }
    pub fn account(&self) -> &Account {
        &self.account
    }
    pub fn account_mut(&mut self) -> &mut Account {
        &mut self.account
    }
    pub fn entry_gate(&self) -> &EntryGate {
        &self.gate
    }
    pub fn screen(&self) -> &Screen {
        &self.screen
    }
    pub fn hosts(&self) -> impl Iterator<Item = (&HostConfig, &HostData)> {
        let ready = matches!(self.gate, EntryGate::Ready { .. });
        self.hosts_file
            .hosts
            .iter()
            .chain(self.relay_configs.values())
            .filter_map(|config| self.hosts.get(&config.id).map(|host| (config, host)))
            .filter(move |_| ready)
    }
    pub fn host(&self, id: &HostId) -> Option<&HostData> {
        self.hosts()
            .find(|(config, _)| &config.id == id)
            .map(|(_, host)| host)
    }
    pub(super) fn host_any(&self, id: &HostId) -> Option<&HostData> {
        self.hosts.get(id)
    }
    pub fn host_mut(&mut self, id: &HostId) -> Option<&mut HostData> {
        if !matches!(self.gate, EntryGate::Ready { .. }) || !self.is_paired(id) {
            return None;
        }
        self.hosts.get_mut(id)
    }
    pub fn host_if_instance(&self, id: &HostId, instance: HostInstance) -> Option<&HostData> {
        self.host(id).filter(|host| host.instance() == instance)
    }
    pub fn source_provider(
        &self,
        id: &HostId,
        instance: HostInstance,
    ) -> Option<super::host::ScopedSourceProvider> {
        self.host_if_instance(id, instance)
            .map(HostData::source_provider)
    }
    pub fn host_mut_if_instance(
        &mut self,
        id: &HostId,
        instance: HostInstance,
    ) -> Option<&mut HostData> {
        self.host_mut(id).filter(|host| host.instance() == instance)
    }
    pub fn machine_live(&self, machine_id: &str) -> bool {
        self.hosts
            .get(&HostId(machine_id.to_owned()))
            .is_some_and(HostData::is_live)
    }
    pub fn is_paired(&self, id: &HostId) -> bool {
        self.relay_configs.contains_key(id)
            || self.hosts_file.hosts.iter().any(|host| &host.id == id)
    }
    pub fn active_host(&self) -> Option<&HostData> {
        self.screen
            .host()
            .and_then(|id| self.host(id))
            .or_else(|| self.hosts_file.last.as_ref().and_then(|id| self.host(id)))
            .or_else(|| self.hosts().next().map(|(_, host)| host))
    }
    fn landing(&self, screen: Screen) -> Screen {
        if !matches!(self.gate, EntryGate::Ready { .. }) {
            return match screen {
                Screen::Settings(SettingsPage::Account) | Screen::Home => screen,
                _ => Screen::Home,
            };
        }
        match (screen, self.active_host()) {
            (Screen::Home, Some(host)) => Screen::NewThread {
                host: host.id.clone(),
                project: None,
            },
            (screen, _) => screen,
        }
    }
    pub fn navigate(&mut self, screen: Screen) {
        if !matches!(self.gate, EntryGate::Ready { .. })
            && !matches!(
                screen,
                Screen::Home | Screen::Settings(SettingsPage::Account)
            )
        {
            self.pending_screen = Some(screen);
            return;
        }
        let screen = self.landing(screen);
        if let Screen::Thread { host, thread } = &screen
            && let Some(host) = self.hosts.get_mut(host)
        {
            host.ensure_thread(thread);
        }
        if screen == self.screen {
            return;
        }
        if let Some(host) = screen.host()
            && self.hosts_file.last.as_ref() != Some(host)
        {
            let mut file = self.hosts_file.clone();
            file.last = Some(host.clone());
            match self.files.save_hosts(&file) {
                Ok(()) => self.hosts_file = file,
                Err(error) => tracing::warn!("couldn't remember the last host: {error:#}"),
            }
        }
        let previous = std::mem::replace(&mut self.screen, screen);
        self.back.push(previous);
        if self.back.len() > 50 {
            self.back.remove(0);
        }
        self.events.push(CoreEvent::ScreenChanged);
    }
    pub fn screen_behind_settings(&self) -> Option<&Screen> {
        self.back
            .iter()
            .rev()
            .find(|screen| !matches!(screen, Screen::Settings(_)))
    }
    pub fn workspace_screen(&self) -> &Screen {
        if matches!(self.screen, Screen::Settings(_)) {
            self.screen_behind_settings().unwrap_or(&self.screen)
        } else {
            &self.screen
        }
    }
    pub fn close_settings(&mut self) {
        if !matches!(self.screen, Screen::Settings(_)) {
            return;
        }
        if let Some(index) = self
            .back
            .iter()
            .rposition(|screen| !matches!(screen, Screen::Settings(_)))
        {
            self.back.truncate(index + 1);
            self.go_back();
            return;
        }
        self.back.clear();
        self.screen = self.landing(Screen::Home);
        self.events.push(CoreEvent::ScreenChanged);
    }
    pub fn go_back(&mut self) {
        if let Some(screen) = self.back.pop() {
            self.screen = self.landing(screen);
            if let Screen::Thread { host, thread } = &self.screen
                && let Some(host) = self.hosts.get_mut(host)
            {
                host.ensure_thread(thread);
            }
            self.events.push(CoreEvent::ScreenChanged);
        }
    }
    pub fn set_setting(&mut self, key: &str, value: serde_json::Value) {
        if self.settings.get(key) == Some(&value) {
            return;
        }
        let previous = self.settings.insert(key.to_owned(), value);
        if let Err(error) = self.files.save_settings(&self.settings) {
            match previous {
                Some(value) => {
                    self.settings.insert(key.to_owned(), value);
                }
                None => {
                    self.settings.remove(key);
                }
            }
            tracing::error!("couldn't save settings: {error:#}");
            return;
        }
        self.events.push(CoreEvent::SettingsChanged);
    }
    pub fn remove_host(&mut self, id: &HostId) -> Result<()> {
        let mut file = self.hosts_file.clone();
        file.hosts.retain(|known| &known.id != id);
        if file.last.as_ref() == Some(id) {
            file.last = None;
        }
        self.files.save_hosts(&file)?;
        self.hosts_file = file;
        self.relay_configs.remove(id);
        if let Some(host) = self.hosts.remove(id) {
            host.client.close_relay();
        }
        self.dials.remove(id);
        if self.screen.host() == Some(id) {
            self.navigate(Screen::Home);
        }
        self.events.push(CoreEvent::HostsChanged);
        Ok(())
    }
    pub fn mcp_sign_in(&self, host: &HostId, server: &str) -> Option<&McpSignIn> {
        self.mcp_sign_ins.get(host, server)
    }
    pub fn start_mcp_sign_in(
        &mut self,
        host: &HostId,
        server: &str,
        open: bool,
    ) -> tokio::sync::oneshot::Receiver<Result<String>> {
        if let Some(client) = self.host_any(host).map(|host| host.client.clone()) {
            return self
                .mcp_sign_ins
                .start(host.clone(), server.to_owned(), client, open);
        }
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let _ = sender.send(Err(anyhow::anyhow!("This Forge is no longer connected.")));
        receiver
    }
    pub fn cancel_mcp_sign_in(&mut self, host: &HostId, server: &str) {
        if let Some(client) = self.host_any(host).map(|host| host.client.clone()) {
            self.mcp_sign_ins.cancel(host, server, client);
        }
    }

    pub fn connect_machine(&mut self, machine_id: &str) {
        let id = HostId(machine_id.to_owned());
        self.dials.remove(&id);
        self.dial(&id);
    }
    pub fn complete_setup(&mut self, machine_id: String) -> Result<()> {
        anyhow::ensure!(
            self.machine_live(&machine_id),
            "connect the Forge first, then finish setup"
        );
        self.account.complete_onboarding(machine_id)?;
        self.reconcile();
        Ok(())
    }
    fn dial(&mut self, id: &HostId) {
        if self.hosts.contains_key(id) || self.dials.contains_key(id) {
            return;
        }
        let Some(profile) = self.account.profile() else {
            return;
        };
        let Some(machine) = access::live_machine(profile, &id.0).cloned() else {
            return;
        };
        let saved = self
            .hosts_file
            .hosts
            .iter()
            .find(|config| config.id == *id)
            .cloned();
        let paired_directly = saved
            .as_ref()
            .is_some_and(|config| matches!(config.kind, HostKind::Local | HostKind::Wsl { .. }));
        let reading = self.account.reading_local_forge();
        if reading && !paired_directly {
            return;
        }
        let direct = matches!(self.account.local_forge(), LocalForge::Enrolled { machine_id } if machine_id == machine.id)
            || (reading && paired_directly);
        let account_id = profile.account_id.clone();
        let generation = self.dial_generations.entry(id.clone()).or_default();
        *generation += 1;
        let generation = *generation;
        let id = id.clone();
        let task_id = id.clone();
        let sender = self.opened.sender();
        let identity = self.identity.clone();
        let ae = self.account.services().ae.clone();
        let route = self.account.services().relay.clone();
        let device_name = self.device_name.clone();
        let http = self.account.http().clone();
        let session = self.account.session();
        let task = runtime().spawn(async move {
            let result = if direct {
                async {
                    let identity = identity.context("this device has no identity to pair with")?;
                    let config = match saved {
                        Some(config) => config,
                        None => {
                            hosts::pair_this_machine(
                                &ae,
                                &identity,
                                &device_name,
                                task_id.clone(),
                                machine.label,
                            )
                            .await?
                        }
                    };
                    let client = ForgeClient::new(&config.address, &config.fingerprint, &identity)?;
                    anyhow::Ok((config, client))
                }
                .await
                .context("connect to this machine's Forge")
            } else {
                let label = machine.label.clone();
                let config = HostConfig {
                    id: task_id.clone(),
                    name: label.clone(),
                    address: String::new(),
                    fingerprint: String::new(),
                    kind: HostKind::Account {
                        account: account_id.clone(),
                        machine: machine.id.clone(),
                    },
                };
                async {
                    let session = session.context("sign in first")?;
                    services::open_relay(route, http, session, account_id, machine)
                        .await
                        .map(|client| (config, client))
                }
                .await
                .with_context(|| format!("connect to {label} through the Relay"))
            };
            let _ = sender
                .send(Opened {
                    id: task_id,
                    generation,
                    result,
                })
                .await;
        });
        self.dials
            .insert(id, Dial::Opening { _task: AbortOnDropHandle::new(task) });
    }
    fn finish_dial(&mut self, opened: Opened) {
        if self.dial_generations.get(&opened.id) != Some(&opened.generation)
            || !self.dials.contains_key(&opened.id)
        {
            return;
        }
        let (config, client) = match opened.result {
            Ok(opened) => opened,
            Err(error) => {
                self.dials.insert(opened.id, Dial::Failed);
                self.account.report(&error);
                return;
            }
        };
        self.dials.remove(&opened.id);
        if matches!(config.kind, HostKind::Account { .. }) {
            self.relay_configs.insert(opened.id.clone(), config.clone());
        } else if !self.hosts_file.hosts.contains(&config) {
            let mut file = self.hosts_file.clone();
            file.hosts.retain(|known| known.id != config.id);
            file.hosts.push(config.clone());
            match self.files.save_hosts(&file) {
                Ok(()) => self.hosts_file = file,
                Err(error) => self
                    .account
                    .report(&error.context("save the pairing with this machine's Forge")),
            }
        }
        let mut host = HostData::new(
            config.id.clone(),
            config.name.clone(),
            client,
            self.wake.sender(),
            self.admission.clone(),
            self.events.clone(),
        );
        host.set_pairing(&config.kind, &config.address);
        if let Some(assets) = &self.image_assets {
            host.set_image_assets(assets.clone());
        }
        self.hosts.insert(opened.id, host);
        self.events.push(CoreEvent::HostsChanged);
    }
    fn drop_hosts(&mut self, ids: &[HostId]) {
        if ids.is_empty() {
            return;
        }
        for id in ids {
            if let Some(host) = self.hosts.remove(id) {
                host.client.close_relay();
            }
            self.relay_configs.remove(id);
            self.dials.remove(id);
        }
        if self.screen.host().is_some_and(|host| ids.contains(host)) {
            self.screen = Screen::Home;
            self.events.push(CoreEvent::ScreenChanged);
        }
        self.back
            .retain(|screen| screen.host().is_none_or(|host| !ids.contains(host)));
        self.pending_screen = self
            .pending_screen
            .take()
            .filter(|screen| screen.host().is_none_or(|host| !ids.contains(host)));
        self.events.push(CoreEvent::HostsChanged);
    }
    fn reconcile(&mut self) {
        let gate = self.account.entry_gate();
        if !matches!(gate, EntryGate::Ready { .. }) {
            for host in self.hosts.values() {
                host.revoke_source_providers();
            }
        }
        self.gate = gate.clone();
        let Some(profile) = self.account.profile().cloned() else {
            let ids: Vec<_> = self.hosts.keys().cloned().collect();
            self.dials.clear();
            self.drop_hosts(&ids);
            self.events.push(CoreEvent::AccessChanged);
            return;
        };
        let reading = self.account.reading_local_forge();
        let local = match self.account.local_forge() {
            LocalForge::Enrolled { machine_id } => Some(machine_id),
            _ => None,
        };
        let onboarded = self.account.onboarded_machine().map(str::to_owned);
        let stale: Vec<_> = self
            .hosts
            .keys()
            .filter(|id| {
                profile
                    .machines
                    .iter()
                    .any(|machine| machine.id == id.0 && machine.revoked_at.is_some())
                    || (self.relay_configs.contains_key(*id)
                        && access::live_machine(&profile, &id.0).is_none())
            })
            .cloned()
            .collect();
        self.drop_hosts(&stale);
        self.dials
            .retain(|id, _| access::live_machine(&profile, &id.0).is_some());
        if let Some(machine_id) = local.filter(|_| !reading) {
            self.dial(&HostId(machine_id));
        }
        if let (EntryGate::Ready { .. }, Some(machine_id)) = (&gate, onboarded) {
            self.dial(&HostId(machine_id));
        }
        if matches!(gate, EntryGate::Ready { .. })
            && let Some(screen) = self.pending_screen.take()
        {
            self.navigate(screen);
        }
        self.events.push(CoreEvent::AccessChanged);
    }
    pub fn drain_with(&mut self, receive: impl FnMut(&mut Core, CoreEvent)) {
        if self.drain_step(receive) {
            self.opened.wake();
        }
    }
    pub fn wake(&self) {
        self.opened.wake();
    }
    pub fn drain_step(&mut self, mut receive: impl FnMut(&mut Core, CoreEvent)) -> bool {
        if self.draining.replace(true) {
            return true;
        }
        let _guard = DrainGuard(self.draining.clone());
        self.wake.drain();
        self.dispatch_events(&mut receive);
        enum Source {
            Account,
            Mcp,
            Opened,
            Host(HostId),
        }
        let admission = self.admission.clone();
        let next = admission.inspect(|| {
            let mut next: Option<(u64, Source)> = None;
            let mut consider = |sequence: Option<u64>, source: Source| {
                if let Some(sequence) = sequence
                    && next.as_ref().is_none_or(|(first, _)| sequence < *first)
                {
                    next = Some((sequence, source));
                }
            };
            consider(self.account.peek_order(), Source::Account);
            consider(self.mcp_sign_ins.peek_order(), Source::Mcp);
            consider(self.opened.peek_order(), Source::Opened);
            for (id, host) in &mut self.hosts {
                consider(host.peek_order(), Source::Host(id.clone()));
            }
            next
        });
        match next.map(|(_, source)| source) {
            Some(Source::Account) => self.deliver_account(&mut receive),
            Some(Source::Mcp) => self.deliver_mcp(&mut receive),
            Some(Source::Opened) => {
                if let Some(opened) = self.opened.drain_one() {
                    self.finish_dial(opened);
                }
                self.dispatch_events(&mut receive);
            }
            Some(Source::Host(id)) => self.deliver_host(&id, &mut receive),
            None => {}
        }
        self.dispatch_events(&mut receive);
        self.admitted_pending()
    }
    fn admitted_pending(&mut self) -> bool {
        let admission = self.admission.clone();
        admission.inspect(|| {
            self.account.peek_order().is_some()
                || self.mcp_sign_ins.peek_order().is_some()
                || self.opened.peek_order().is_some()
                || self
                    .hosts
                    .values_mut()
                    .any(|host| host.peek_order().is_some())
        })
    }
    fn deliver_account(&mut self, receive: &mut impl FnMut(&mut Core, CoreEvent)) {
        let acknowledgement = self.account.drain();
        self.dispatch_events(receive);
        drop(acknowledgement);
    }
    fn deliver_mcp(&mut self, receive: &mut impl FnMut(&mut Core, CoreEvent)) {
        self.mcp_sign_ins.drain();
        self.dispatch_events(receive);
    }
    fn deliver_host(&mut self, id: &HostId, receive: &mut impl FnMut(&mut Core, CoreEvent)) {
        if let Some(host) = self.hosts.get_mut(id) {
            host.drain();
        }
        self.dispatch_events(receive);
    }
    fn dispatch_events(&mut self, receive: &mut impl FnMut(&mut Core, CoreEvent)) {
        while let Some(effect) = self.events.pop() {
            match effect {
                Effect::Core(event) => receive(self, event),
                Effect::Account(event) => {
                    if event == AccountEvent::Synced {
                        self.dials.retain(|_, dial| !matches!(dial, Dial::Failed));
                    }
                    self.reconcile();
                    receive(self, CoreEvent::Account(event));
                }
                Effect::Mcp(event) => receive(
                    self,
                    match event {
                        McpEvent::Changed => CoreEvent::McpSignInChanged,
                        McpEvent::OpenUrl(url) => CoreEvent::OpenUrl(url),
                    },
                ),
                Effect::Host(id, instance, event) => {
                    if self
                        .host_any(&id)
                        .is_none_or(|host| host.instance() != instance)
                    {
                        continue;
                    }
                    if let HostEvent::WslDistro { distro } = &event {
                        let mut file = self.hosts_file.clone();
                        if let Some(config) = file.hosts.iter_mut().find(|config| config.id == id) {
                            config.kind = HostKind::Wsl {
                                distro: distro.clone(),
                            };
                            match self.files.save_hosts(&file) {
                                Ok(()) => self.hosts_file = file,
                                Err(error) => tracing::warn!(
                                    "couldn't save the host's WSL distribution: {error:#}"
                                ),
                            }
                        }
                    }
                    if let HostEvent::EditorCall { id: call_id, call } = &event {
                        self.editor_call(&id, call_id.clone(), call.clone());
                    }
                    if matches!(event, HostEvent::Changed)
                        && let Some(host) = self.hosts.get_mut(&id)
                    {
                        let live = host.is_live();
                        if live != host.reported_live {
                            host.reported_live = live;
                            self.events.push(CoreEvent::HostsChanged);
                        }
                    }
                    receive(self, CoreEvent::Host { host: id, event });
                }
            }
        }
    }
    pub fn drain(&mut self) -> Vec<CoreEvent> {
        let mut events = Vec::new();
        self.drain_with(|_, event| events.push(event));
        events
    }
}
fn device_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|name| name.trim().to_owned())
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Artisan Editor".to_owned())
}
