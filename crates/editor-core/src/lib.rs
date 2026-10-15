//! The Editor's connection to its Forges, and navigation.
//!
//! [`Core`] owns the paired hosts and the current [`Screen`]. Each host's
//! [`HostData`] is what that Forge has sent, updated only by its event
//! stream. Sections read from these and call Forge through
//! [`ForgeClient`]; nothing else is shared between sections.

mod client;
mod connection;
mod content_text;
mod content_window;
mod host;
pub mod hosts;
pub mod media_http;
mod pairing_targets;
mod resident;
pub mod runtime;
mod screen;
pub use pairing_targets::PairingTargets;
pub use resident::{CollectionBytes, ResidentBytes};

pub use client::ForgeClient;
pub use connection::ConnectionStatus;
pub use content_text::ContentText;
pub use content_window::{ContentWindow, ContentWindowEvent, ContentWindowState};
pub use host::{
    BlobImage, HostData, HostEvent, LiveSnapshot, ProjectionLoad, ThreadPage, ThreadPageDirection,
    ThreadSummary,
};
pub use hosts::{EditorFiles, EditorSettings, HostConfig, HostId, HostKind, LocalTarget};
pub use ropey::Rope;
pub use screen::{Screen, SettingsPage};

use anyhow::{Context as _, Result};
use artisan_api::tls::Identity;
use gpui::{App, AppContext as _, Context, Entity, EventEmitter};
use hosts::HostsFile;
use std::collections::HashMap;

pub struct Core {
    pub files: EditorFiles,
    pub identity: Option<Identity>,
    pub settings: EditorSettings,
    hosts: HashMap<HostId, Entity<HostData>>,
    hosts_file: HostsFile,
    pairing_targets: PairingTargets,
    screen: Screen,
    back: Vec<Screen>,
    /// The device name sent when pairing.
    pub device_name: String,
}

#[derive(Clone, Debug)]
pub enum CoreEvent {
    ScreenChanged,
    HostsChanged,
    SettingsChanged,
}

impl EventEmitter<CoreEvent> for Core {}

impl Core {
    /// Loads this device's files and connects to every paired host.
    pub fn new(files: EditorFiles, cx: &mut Context<Self>) -> Result<Self> {
        let identity = files.identity()?;
        let hosts_file = files.hosts()?;
        let settings = files.settings().unwrap_or_else(|error| {
            tracing::warn!("ignored unreadable settings: {error:#}");
            EditorSettings::new()
        });
        let mut core = Self {
            files,
            identity: Some(identity.clone()),
            settings,
            hosts: HashMap::new(),
            hosts_file,
            pairing_targets: PairingTargets::default(),
            screen: Screen::Home,
            back: Vec::new(),
            device_name: device_name(),
        };
        for config in core.hosts_file.hosts.clone() {
            let client = ForgeClient::new(&config.address, &config.fingerprint, &identity);
            if let Err(error) = client.map(|client| core.connect(&config, client, cx)) {
                tracing::error!(host = %config.id, "can't connect to {}: {error:#}", config.name);
            }
        }
        if let Some(reduced) = core
            .settings
            .get("reduced_motion")
            .and_then(serde_json::Value::as_bool)
        {
            cx.set_reduce_motion(reduced);
        }
        Ok(core)
    }

    fn connect(
        &mut self,
        config: &HostConfig,
        client: ForgeClient,
        cx: &mut Context<Self>,
    ) -> Entity<HostData> {
        let (id, name) = (config.id.clone(), config.name.clone());
        let host = cx.new(|cx| HostData::new(id, name, client, cx));
        self.hosts.insert(config.id.clone(), host.clone());
        host
    }

    pub fn hosts(&self) -> impl Iterator<Item = (&HostConfig, &Entity<HostData>)> {
        self.hosts_file
            .hosts
            .iter()
            .filter_map(|config| self.hosts.get(&config.id).map(|host| (config, host)))
    }

    #[must_use]
    pub fn host(&self, id: &HostId) -> Option<&Entity<HostData>> {
        self.hosts.get(id)
    }

    /// True until the user explicitly removes this pairing, including connection failures.
    pub fn is_paired(&self, id: &HostId) -> bool {
        self.hosts_file.hosts.iter().any(|host| &host.id == id)
    }

    /// The current screen's host, then the saved last host, then the first paired host.
    #[must_use]
    pub fn active_host(&self) -> Option<&Entity<HostData>> {
        self.screen
            .host()
            .and_then(|id| self.host(id))
            .or_else(|| self.hosts_file.last.as_ref().and_then(|id| self.host(id)))
            .or_else(|| self.hosts().next().map(|(_, host)| host))
    }

    /// Saves a newly paired host and connects to it.
    pub fn add_host(&mut self, config: HostConfig, cx: &mut Context<Self>) -> Result<()> {
        let identity = self.identity.as_ref().context(
            "this device's identity could not be loaded; repair its configuration before pairing",
        )?;
        let client = ForgeClient::new(&config.address, &config.fingerprint, identity)?;
        let mut file = self.hosts_file.clone();
        file.hosts.retain(|known| known.id != config.id);
        file.hosts.push(config.clone());
        file.last = Some(config.id.clone());
        self.files.save_hosts(&file)?;
        self.hosts_file = file;
        self.connect(&config, client, cx);
        cx.emit(CoreEvent::HostsChanged);
        cx.notify();
        Ok(())
    }

    pub fn remove_host(&mut self, id: &HostId, cx: &mut Context<Self>) -> Result<()> {
        let mut file = self.hosts_file.clone();
        file.hosts.retain(|known| &known.id != id);
        if file.last.as_ref() == Some(id) {
            file.last = None;
        }
        self.files.save_hosts(&file)?;
        self.hosts_file = file;
        self.hosts.remove(id);
        if self.screen.host() == Some(id) {
            self.navigate(Screen::Home, cx);
        }
        cx.emit(CoreEvent::HostsChanged);
        cx.notify();
        Ok(())
    }

    #[must_use]
    pub fn screen(&self) -> &Screen {
        &self.screen
    }

    pub fn navigate(&mut self, screen: Screen, cx: &mut Context<Self>) {
        if let Screen::Thread { host, thread } = &screen {
            if let Some(host) = self.host(host).cloned() {
                host.update(cx, |host, cx| host.ensure_thread(thread, cx));
            }
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
        cx.emit(CoreEvent::ScreenChanged);
        cx.notify();
    }

    pub fn go_back(&mut self, cx: &mut Context<Self>) {
        if let Some(screen) = self.back.pop() {
            if let Screen::Thread { host, thread } = &screen {
                if let Some(host) = self.host(host).cloned() {
                    host.update(cx, |host, cx| host.ensure_thread(thread, cx));
                }
            }
            self.screen = screen;
            cx.emit(CoreEvent::ScreenChanged);
            cx.notify();
        }
    }

    /// Stores a device-local preference.
    pub fn set_setting(&mut self, key: &str, value: serde_json::Value, cx: &mut Context<Self>) {
        if self.settings.get(key) == Some(&value) {
            return;
        }
        let previous = self.settings.insert(key.to_owned(), value);
        if let Err(error) = self.files.save_settings(&self.settings) {
            match previous {
                Some(previous) => {
                    self.settings.insert(key.to_owned(), previous);
                }
                None => {
                    self.settings.remove(key);
                }
            }
            tracing::error!("couldn't save settings: {error:#}");
            cx.notify();
            return;
        }
        if key == "reduced_motion" {
            cx.set_reduce_motion(
                self.settings
                    .get(key)
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
            );
        }
        cx.emit(CoreEvent::SettingsChanged);
        cx.notify();
    }
}

/// Creates the [`Core`] entity. If this device's files can't be read it
/// still returns a usable (empty) core, with the error to show.
pub fn init(cx: &mut App) -> (Entity<Core>, Option<anyhow::Error>) {
    let files = EditorFiles::default_location();
    let mut failure = None;
    let core = cx.new(|cx| match Core::new(files.clone(), cx) {
        Ok(core) => core,
        Err(error) => {
            failure = Some(error);
            Core {
                files,
                identity: None,
                settings: EditorSettings::new(),
                hosts: HashMap::new(),
                hosts_file: HostsFile::default(),
                pairing_targets: PairingTargets::default(),
                screen: Screen::Home,
                back: Vec::new(),
                device_name: device_name(),
            }
        }
    });
    (core, failure)
}

#[cfg(test)]
mod tests;

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

mod collection_window;
pub use collection_window::{CollectionWindow, CollectionWindowEvent, CollectionWindowState};

mod collection_browser;
pub use collection_browser::{
    CollectionActivation, CollectionBrowser, CollectionPresenter, CollectionPreview,
};
