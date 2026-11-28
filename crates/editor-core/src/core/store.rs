use crate::core::{
    ForgeClient,
    access::{self, EntryGate},
    account::{Account, AccountEvent, LocalForge},
    client::NewClientError,
    config_file::{ReadConfigError, UpdateConfigError},
    delivery::{AdmissionOrder, Delivery},
    host::{HostData, HostEvent, HostInstance},
    hosts::{self, EditorFiles, HostConfig, HostId, HostKind, HostsFile, IdentityError},
    mcp_sign_in::{McpEvent, McpSignIn, McpSignIns},
    runtime::runtime,
    screen::{Screen, SettingsPage},
    services::{self, Services},
    settings::{SetSettingError, Settings},
    settings_writer::{SettingsSave, SettingsSaves},
};
use aqua::mailbox::SendError;
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
    result: Result<(HostConfig, ForgeClient), DialError>,
}

#[derive(Debug, thiserror::Error)]
pub enum DialError {
    #[error("this device has no identity to pair with {host}")]
    NoIdentity { host: HostId },
    #[error("pair with this machine's Forge")]
    Pair(#[source] hosts::PairThisMachineError),
    #[error("connect to this machine's Forge {host}")]
    Client {
        host: HostId,
        #[source]
        source: NewClientError,
    },
    #[error("sign in first to connect to {label} through the Relay")]
    SignedOut { host: HostId, label: String },
    #[error("connect to {label} through the Relay")]
    Relay {
        host: HostId,
        label: String,
        #[source]
        source: Box<services::OpenRelayError>,
    },
    #[error("save the pairing with this machine's Forge {host}")]
    Save {
        host: HostId,
        #[source]
        source: UpdateConfigError,
    },
}

#[derive(Debug, thiserror::Error)]
#[error("forget the pairing with {host}")]
pub struct RemoveHostError {
    host: HostId,
    #[source]
    source: UpdateConfigError,
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
            queue: std::rc::Rc::default(),
            wake,
            draining,
        }
    }
    fn enqueue(&self, effect: Effect) {
        self.queue.borrow_mut().push_back(effect);
        if !self.draining.get() {
            match self.wake.send(()) {
                Ok(()) | Err(SendError::Full(()) | SendError::Closed(())) => {}
            }
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
}

pub struct Core {
    files: EditorFiles,
    identity: Option<Identity>,
    pub(crate) settings: Settings,
    device_name: String,
    hosts: HashMap<HostId, HostData>,
    hosts_file: HostsFile,
    relay_configs: HashMap<HostId, HostConfig>,
    screen: Screen,
    workspace_host: Option<HostId>,
    back: Vec<Screen>,
    pending_screen: Option<Screen>,
    gate: EntryGate,
    account: Account,
    image_assets: Option<aqua::images::ImageAssets>,
    mcp_sign_ins: McpSignIns,
    wake: aqua::mailbox::Mailbox<()>,
    opened: Delivery<Opened>,
    saves: SettingsSaves,
    pub(super) setting_calls: Vec<crate::core::agent_calls::SettingCall>,
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

#[derive(Debug, thiserror::Error)]
pub enum OpenCoreError {
    #[error("open this device's identity")]
    Identity(#[source] IdentityError),
    #[error("read the paired Forges")]
    Hosts(#[source] ReadConfigError),
}

impl Core {
    pub fn clock(&self) -> &crate::core::clock::Clock {
        &self.account.services().clock
    }
    pub fn with_services(files: EditorFiles, services: Services) -> Result<Self, OpenCoreError> {
        let identity = files.identity().map_err(OpenCoreError::Identity)?;
        let hosts_file = files.hosts().map_err(OpenCoreError::Hosts)?;
        Ok(Self::assemble(files, services, Some(identity), hosts_file))
    }
    pub fn init(files: EditorFiles) -> (Self, Vec<OpenCoreError>) {
        let mut failures = Vec::new();
        let identity = match files.identity() {
            Ok(identity) => Some(identity),
            Err(error) => {
                failures.push(OpenCoreError::Identity(error));
                None
            }
        };
        let hosts_file = match files.hosts() {
            Ok(hosts) => hosts,
            Err(error) => {
                failures.push(OpenCoreError::Hosts(error));
                HostsFile::default()
            }
        };
        for failure in &failures {
            tracing::error!(error = %::diagnostics::chain(failure), "opening the Editor's files");
        }
        (
            Self::assemble(files, Services::installed(), identity, hosts_file),
            failures,
        )
    }
    fn assemble(
        files: EditorFiles,
        services: Services,
        identity: Option<Identity>,
        hosts_file: HostsFile,
    ) -> Self {
        let settings = Settings::read(files.settings());
        if let Some(error) = settings.unreadable() {
            tracing::error!(
                error = %::diagnostics::chain(error),
                "settings.json is unreadable; keeping it and blocking saves until it is recovered"
            );
        }
        let wake = aqua::mailbox::Mailbox::new(NonZeroUsize::MIN);
        let admission = AdmissionOrder::default();
        let draining = std::rc::Rc::new(std::cell::Cell::new(false));
        let events = Effects::new(wake.sender(), std::rc::Rc::clone(&draining));
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
            NonZeroUsize::new(64).expect("invariant: the dial delivery capacity is non-zero"),
            wake.sender(),
            admission.clone(),
        );
        let saves = SettingsSaves::new(files.clone(), wake.sender(), admission.clone(), &settings);
        let mut core = Self {
            files,
            identity,
            settings,
            device_name: device_name(),
            hosts: HashMap::new(),
            hosts_file,
            relay_configs: HashMap::new(),
            screen: Screen::Home,
            workspace_host: None,
            back: Vec::new(),
            pending_screen: None,
            gate,
            account,
            image_assets: None,
            mcp_sign_ins,
            wake,
            opened,
            saves,
            setting_calls: Vec::new(),
            admission,
            dials: HashMap::new(),
            dial_generations: HashMap::new(),
            events,
            draining,
        };
        core.reconcile();
        core
    }
    pub fn set_image_assets(&mut self, assets: aqua::images::ImageAssets) {
        self.hosts
            .values_mut()
            .for_each(|host| host.set_image_assets(assets.clone()));
        self.image_assets = Some(assets);
    }
    pub fn wake_mailbox(&self) -> &aqua::mailbox::Mailbox<()> {
        &self.wake
    }

    pub fn pending_work(&self) -> usize {
        let admission = self.admission.clone();
        admission.inspect(|| {
            let admitted = self.account.admitted()
                + self.mcp_sign_ins.admitted()
                + self.opened.admitted()
                + self.saves.admitted()
                + self.hosts.values().map(HostData::admitted).sum::<usize>();
            let opening = self
                .dials
                .values()
                .filter(|dial| matches!(dial, Dial::Opening { .. }))
                .count();
            admitted + opening + self.events.queue.borrow().len()
        })
    }
    pub const fn files(&self) -> &EditorFiles {
        &self.files
    }
    pub const fn identity(&self) -> Option<&Identity> {
        self.identity.as_ref()
    }
    pub const fn settings(&self) -> &Settings {
        &self.settings
    }
    pub fn device_name(&self) -> &str {
        &self.device_name
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
    pub fn machine_connecting(&self, machine_id: &str) -> bool {
        let id = HostId(machine_id.to_owned());
        matches!(self.dials.get(&id), Some(Dial::Opening { .. }))
            || self.hosts.get(&id).is_some_and(|host| {
                matches!(
                    host.status,
                    super::ConnectionStatus::Connecting
                        | super::ConnectionStatus::Reconnecting { .. }
                )
            })
    }

    pub fn machine_uses_relay(&self, machine_id: &str) -> bool {
        self.relay_configs
            .contains_key(&HostId(machine_id.to_owned()))
    }

    fn is_paired(&self, id: &HostId) -> bool {
        self.relay_configs.contains_key(id)
            || self.hosts_file.hosts.iter().any(|host| &host.id == id)
    }
    pub fn active_host(&self) -> Option<&HostData> {
        self.workspace_host
            .as_ref()
            .and_then(|id| self.host(id))
            .or_else(|| self.hosts_file.last.as_ref().and_then(|id| self.host(id)))
            .or_else(|| self.hosts().next().map(|(_, host)| host))
    }
    fn show(&mut self, screen: Screen) -> Screen {
        if let Some(host) = screen.host() {
            self.workspace_host = Some(host.clone());
            if self.hosts_file.last.as_ref() != Some(host) {
                let last = host.clone();
                match self.files.update_hosts(|file| file.last = Some(last)) {
                    Ok(file) => self.hosts_file = file,
                    Err(error) => tracing::warn!(
                        host = %host,
                        error = %::diagnostics::chain(&error),
                        "couldn't remember the last host"
                    ),
                }
            }
        }
        std::mem::replace(&mut self.screen, screen)
    }
    fn landing(&self, screen: Screen) -> Screen {
        if !matches!(self.gate, EntryGate::Ready { .. }) {
            return match screen {
                Screen::Settings(SettingsPage::Account) | Screen::Home => screen,
                Screen::Thread { .. }
                | Screen::NewThread { .. }
                | Screen::Settings(_)
                | Screen::Marketplace
                | Screen::Usage => Screen::Home,
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
        let previous = self.show(screen);
        self.back.push(previous);
        if self.back.len() > 50 {
            self.back.remove(0);
        }
        self.events.push(CoreEvent::ScreenChanged);
    }
    pub fn show_behind_settings(&mut self, screen: Screen) {
        if !matches!(self.screen, Screen::Settings(_)) {
            self.navigate(screen);
            return;
        }
        if let Screen::Thread { host, thread } = &screen
            && let Some(host) = self.hosts.get_mut(host)
        {
            host.ensure_thread(thread);
        }
        match self
            .back
            .iter()
            .rposition(|shown| !matches!(shown, Screen::Settings(_)))
        {
            Some(index) => self.back[index] = screen,
            None => self.back.insert(0, screen),
        }
        self.events.push(CoreEvent::ScreenChanged);
    }
    fn screen_behind_settings(&self) -> Option<&Screen> {
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
        let landing = self.landing(Screen::Home);
        self.show(landing);
        self.events.push(CoreEvent::ScreenChanged);
    }
    pub fn go_back(&mut self) {
        if let Some(screen) = self.back.pop() {
            let landing = self.landing(screen);
            self.show(landing);
            if let Screen::Thread { host, thread } = &self.screen
                && let Some(host) = self.hosts.get_mut(host)
            {
                host.ensure_thread(thread);
            }
            self.events.push(CoreEvent::ScreenChanged);
        }
    }
    pub fn set_setting(
        &mut self,
        key: &str,
        value: serde_json::Value,
    ) -> Result<(), SetSettingError> {
        self.put_setting(key, value).map(drop)
    }
    pub(crate) fn put_setting(
        &mut self,
        key: &str,
        value: serde_json::Value,
    ) -> Result<Option<u64>, SetSettingError> {
        let (key, value) = match (key, value) {
            ("dark_mode", serde_json::Value::Bool(dark))
                if self.settings.get("theme").is_some() =>
            {
                (
                    "theme",
                    super::ThemePreference::from_dark(dark).as_str().into(),
                )
            }
            (key, value) => (key, value),
        };
        if let Settings::Unreadable(source) = &self.settings {
            return Err(SetSettingError::Unreadable {
                key: key.to_owned(),
                source: std::sync::Arc::clone(source),
            });
        }
        if self.settings.get(key) == Some(&value) && self.settings.rejected(key).is_none() {
            return Ok(self.saves.pending_revision(key));
        }
        crate::core::settings::check(key, &value).map_err(|source| SetSettingError::Invalid {
            key: key.to_owned(),
            source,
        })?;
        self.settings.put(key, Some(value.clone()));
        let revision = self.saves.submit(key.to_owned(), value);
        self.events.push(CoreEvent::SettingsChanged);
        Ok(Some(revision))
    }
    pub const fn settings_save(&self) -> &SettingsSave {
        self.saves.state()
    }
    pub fn reload_settings(&mut self) {
        self.settings = Settings::read(self.files.settings());
        self.saves.reload(&self.settings);
        if let Some(error) = self.settings.unreadable() {
            tracing::warn!(
                error = %::diagnostics::chain(error),
                "settings.json is still unreadable"
            );
        }
        self.events.push(CoreEvent::SettingsChanged);
    }
    pub fn remove_host(&mut self, id: &HostId) -> Result<(), RemoveHostError> {
        let file = self
            .files
            .update_hosts(|file| {
                file.hosts.retain(|known| &known.id != id);
                if file.last.as_ref() == Some(id) {
                    file.last = None;
                }
            })
            .map_err(|source| RemoveHostError {
                host: id.clone(),
                source,
            })?;
        self.hosts_file = file;
        self.relay_configs.remove(id);
        self.mcp_sign_ins.end_host(id);
        if let Some(host) = self.hosts.remove(id) {
            host.close();
        }
        self.dials.remove(id);
        self.forget_navigation_to(std::slice::from_ref(id));
        self.events.push(CoreEvent::HostsChanged);
        Ok(())
    }
    pub fn mcp_sign_in(&self, host: &HostId, server: &str) -> Option<&McpSignIn> {
        let instance = self.host_any(host)?.instance();
        self.mcp_sign_ins.get(host, instance, server)
    }
    pub fn start_mcp_sign_in(
        &mut self,
        host: &HostId,
        server: &str,
        open: bool,
    ) -> tokio::sync::oneshot::Receiver<Result<String, crate::core::mcp_sign_in::StartMcpSignInError>>
    {
        if let Some((instance, client)) = self
            .host_any(host)
            .map(|host| (host.instance(), host.client.clone()))
        {
            return self.mcp_sign_ins.start(
                host.clone(),
                instance,
                server.to_owned(),
                client,
                open,
            );
        }
        let (sender, receiver) = tokio::sync::oneshot::channel();
        sender
            .send(Err(
                crate::core::mcp_sign_in::StartMcpSignInError::Disconnected {
                    host: host.clone(),
                    server: server.to_owned(),
                },
            ))
            .expect("invariant: the sign-in answer's receiver is still held here");
        receiver
    }
    pub fn cancel_mcp_sign_in(&mut self, host: &HostId, server: &str) {
        if let Some(client) = self.host_any(host).map(|host| host.client.clone()) {
            self.mcp_sign_ins.cancel(host, server, client);
        }
    }

    pub fn connect_machine(&mut self, machine_id: &str) {
        let id = HostId(machine_id.to_owned());
        if let Some(host) = self.hosts.get(&id) {
            host.retry_now();
            return;
        }
        self.dials.remove(&id);
        self.dial(&id);
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
                    let identity = identity.ok_or_else(|| DialError::NoIdentity {
                        host: task_id.clone(),
                    })?;
                    let config = match saved {
                        Some(config) => config,
                        None => hosts::pair_this_machine(
                            &ae,
                            &identity,
                            &device_name,
                            task_id.clone(),
                            machine.label,
                        )
                        .await
                        .map_err(DialError::Pair)?,
                    };
                    let client = ForgeClient::new(&config.address, &config.fingerprint, &identity)
                        .map_err(|source| DialError::Client {
                            host: task_id.clone(),
                            source,
                        })?;
                    Ok((config, client))
                }
                .await
            } else {
                open_through_relay(task_id.clone(), machine, account_id, session, route, http).await
            };
            sender
                .offer(Opened {
                    id: task_id,
                    generation,
                    result,
                })
                .await;
        });
        self.dials.insert(
            id,
            Dial::Opening {
                _task: AbortOnDropHandle::new(task),
            },
        );
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
                self.account
                    .report("Couldn't connect to this Forge.", &error);
                return;
            }
        };
        if matches!(config.kind, HostKind::Account { .. }) {
            self.relay_configs.insert(opened.id.clone(), config.clone());
        } else if !self.hosts_file.hosts.contains(&config) {
            let saved = self.files.update_hosts(|file| {
                file.hosts.retain(|known| known.id != config.id);
                file.hosts.push(config.clone());
            });
            match saved {
                Ok(file) => self.hosts_file = file,
                Err(source) => {
                    let error = DialError::Save {
                        host: opened.id.clone(),
                        source,
                    };
                    tracing::error!(
                        host = %opened.id,
                        error = %::diagnostics::chain(&error),
                        "couldn't save the pairing with this machine's Forge"
                    );
                    self.dials.insert(opened.id, Dial::Failed);
                    self.account.report(
                        "Couldn't save the pairing with this machine's Forge.",
                        &error,
                    );
                    return;
                }
            }
        }
        self.dials.remove(&opened.id);
        let mut host = HostData::new(
            config.id.clone(),
            config.name.clone(),
            client,
            self.wake.sender(),
            self.admission.clone(),
            self.events.clone(),
            self.account.services().clock.clone(),
        );
        host.set_pairing(&config.kind, &config.address);
        if let Some(assets) = &self.image_assets {
            host.set_image_assets(assets.clone());
        }
        if self.hosts.insert(opened.id.clone(), host).is_some() {
            self.mcp_sign_ins.end_host(&opened.id);
        }
        self.events.push(CoreEvent::HostsChanged);
    }
    fn drop_hosts(&mut self, ids: &[HostId]) {
        if ids.is_empty() {
            return;
        }
        for id in ids {
            self.mcp_sign_ins.end_host(id);
            if let Some(host) = self.hosts.remove(id) {
                host.close();
            }
            self.relay_configs.remove(id);
            self.dials.remove(id);
        }
        self.forget_navigation_to(ids);
        self.events.push(CoreEvent::HostsChanged);
    }
    fn forget_navigation_to(&mut self, ids: &[HostId]) {
        let elsewhere = |screen: &Screen| screen.host().is_none_or(|host| !ids.contains(host));
        self.back.retain(elsewhere);
        self.pending_screen = self.pending_screen.take().filter(elsewhere);
        if self
            .workspace_host
            .as_ref()
            .is_some_and(|host| ids.contains(host))
        {
            self.workspace_host = None;
        }
        if !elsewhere(&self.screen) {
            let landing = self.landing(Screen::Home);
            self.show(landing);
            self.events.push(CoreEvent::ScreenChanged);
        }
    }
    fn reconcile(&mut self) {
        let gate = self.account.entry_gate();
        if !matches!(gate, EntryGate::Ready { .. }) {
            self.hosts
                .values()
                .for_each(HostData::revoke_source_providers);
            self.mcp_sign_ins.end_all();
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
            LocalForge::Unknown
            | LocalForge::Unenrolled
            | LocalForge::Pending
            | LocalForge::Claimed { .. }
            | LocalForge::Revoked { .. }
            | LocalForge::OtherAccount => None,
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
        if matches!(gate, EntryGate::Ready { .. }) {
            let chosen = onboarded.filter(|id| access::live_machine(&profile, id).is_some());
            let machines: Vec<String> = match chosen {
                Some(machine_id) => vec![machine_id],
                None => access::live_machines(&profile)
                    .map(|machine| machine.id.clone())
                    .collect(),
            };
            for machine_id in machines {
                self.dial(&HostId(machine_id));
            }
        }
        if matches!(gate, EntryGate::Ready { .. })
            && let Some(screen) = self.pending_screen.take()
        {
            self.navigate(screen);
        }
        self.events.push(CoreEvent::AccessChanged);
    }
    pub fn drain_with(&mut self, receive: impl FnMut(&mut Self, CoreEvent)) {
        if self.drain_step(receive) {
            self.opened.wake();
        }
    }
    pub fn wake(&self) {
        self.opened.wake();
    }
    pub fn drain_step(&mut self, mut receive: impl FnMut(&mut Self, CoreEvent)) -> bool {
        if self.draining.replace(true) {
            return true;
        }
        let _guard = DrainGuard(std::rc::Rc::clone(&self.draining));
        self.wake.drain();
        self.dispatch_events(&mut receive);
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
            consider(self.saves.peek_order(), Source::Saved);
            self.hosts
                .iter_mut()
                .for_each(|(id, host)| consider(host.peek_order(), Source::Host(id.clone())));
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
            Some(Source::Saved) => {
                if let Some(settled) = self.saves.settle(&mut self.settings) {
                    if settled.changed {
                        self.events.push(CoreEvent::SettingsChanged);
                    }
                    self.answer_setting_calls(&settled);
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
                || self.saves.peek_order().is_some()
                || self
                    .hosts
                    .values_mut()
                    .any(|host| host.peek_order().is_some())
        })
    }
    fn deliver_account(&mut self, receive: &mut impl FnMut(&mut Self, CoreEvent)) {
        let acknowledgement = self.account.drain();
        self.dispatch_events(receive);
        drop(acknowledgement);
    }
    fn deliver_mcp(&mut self, receive: &mut impl FnMut(&mut Self, CoreEvent)) {
        self.mcp_sign_ins.drain();
        self.dispatch_events(receive);
    }
    fn deliver_host(&mut self, id: &HostId, receive: &mut impl FnMut(&mut Self, CoreEvent)) {
        if let Some(host) = self.hosts.get_mut(id) {
            host.drain();
        }
        self.dispatch_events(receive);
    }
    fn dispatch_events(&mut self, receive: &mut impl FnMut(&mut Self, CoreEvent)) {
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
                    if let HostEvent::WslDistro { distro } = &event
                        && self.hosts_file.hosts.iter().any(|config| config.id == id)
                    {
                        let saved = self.files.update_hosts(|file| {
                            if let Some(config) =
                                file.hosts.iter_mut().find(|config| config.id == id)
                            {
                                config.kind = HostKind::Wsl {
                                    distro: distro.clone(),
                                };
                            }
                        });
                        match saved {
                            Ok(file) => self.hosts_file = file,
                            Err(error) => tracing::warn!(
                                host = %id,
                                error = %::diagnostics::chain(&error),
                                "couldn't save the host's WSL distribution"
                            ),
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
    ["COMPUTERNAME", "HOSTNAME"]
        .into_iter()
        .find_map(device_name_variable)
        .or_else(device_name_file)
        .unwrap_or_else(|| "Artisan Editor".to_owned())
}

fn device_name_variable(key: &'static str) -> Option<String> {
    match std::env::var(key) {
        Ok(name) => Some(name).filter(|name| !name.trim().is_empty()),
        Err(std::env::VarError::NotPresent) => None,
        Err(error @ std::env::VarError::NotUnicode(_)) => {
            tracing::warn!(key, %error, "read the device name from the environment");
            None
        }
    }
}

fn device_name_file() -> Option<String> {
    match std::fs::read_to_string("/etc/hostname") {
        Ok(name) => Some(name.trim().to_owned()).filter(|name| !name.is_empty()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            tracing::warn!(%error, "read the device name from /etc/hostname");
            None
        }
    }
}

enum Source {
    Account,
    Mcp,
    Opened,
    Saved,
    Host(HostId),
}

async fn open_through_relay(
    id: HostId,
    machine: artisan_account::network::MachineEntry,
    account_id: String,
    session: Option<artisan_account::network::SessionToken>,
    route: services::RelayRoute,
    http: artisan_account::network::CoreSessionClient,
) -> Result<(HostConfig, ForgeClient), DialError> {
    let label = machine.label.clone();
    let config = HostConfig {
        id: id.clone(),
        name: label.clone(),
        address: String::new(),
        fingerprint: String::new(),
        kind: HostKind::Account {
            account: account_id.clone(),
            machine: machine.id.clone(),
        },
    };
    match session {
        None => Err(DialError::SignedOut {
            host: id.clone(),
            label,
        }),
        Some(session) => services::open_relay(route, http, session, account_id, machine)
            .await
            .map(|client| (config, client))
            .map_err(|source| DialError::Relay {
                host: id,
                label,
                source: Box::new(source),
            }),
    }
}

impl std::fmt::Debug for Core {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Core")
            .field("hosts", &self.hosts.len())
            .field("workspace_host", &self.workspace_host)
            .field("account", &self.account)
            .finish_non_exhaustive()
    }
}
