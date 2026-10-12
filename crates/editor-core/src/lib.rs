//! The Editor's connection to its Forges, and navigation.
//!
//! [`Core`] owns the paired hosts and the current [`Screen`]. Each host's
//! [`HostData`] is what that Forge has sent, updated only by its event
//! stream. Sections read from these and call Forge through
//! [`ForgeClient`]; nothing else is shared between sections.

mod client;
mod connection;
mod host;
pub mod hosts;
pub mod runtime;
mod screen;

pub use client::ForgeClient;
pub use connection::ConnectionStatus;
pub use host::{HostData, HostEvent};
pub use hosts::{EditorFiles, EditorSettings, HostConfig, HostId, HostKind, LocalTarget};
pub use screen::{Screen, SettingsPage};

use anyhow::Result;
use artisan_api::tls::Identity;
use gpui::{App, AppContext as _, Context, Entity, EventEmitter};

pub struct Core {
    pub files: EditorFiles,
    pub identity: Identity,
    pub settings: EditorSettings,
    hosts: Vec<(HostConfig, Entity<HostData>)>,
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
            identity,
            settings,
            hosts: Vec::new(),
            screen: Screen::Home,
            back: Vec::new(),
            device_name: device_name(),
        };
        for config in hosts_file.hosts {
            if let Err(error) = core.connect(config.clone(), cx) {
                tracing::error!(host = %config.id, "can't connect to {}: {error:#}", config.name);
            }
        }
        Ok(core)
    }

    fn connect(&mut self, config: HostConfig, cx: &mut Context<Self>) -> Result<Entity<HostData>> {
        let client = ForgeClient::new(&config.address, &config.fingerprint, &self.identity)?;
        let (id, name) = (config.id.clone(), config.name.clone());
        let host = cx.new(|cx| HostData::new(id, name, client, cx));
        self.hosts.retain(|(known, _)| known.id != config.id);
        self.hosts.push((config, host.clone()));
        Ok(host)
    }

    pub fn hosts(&self) -> impl Iterator<Item = (&HostConfig, &Entity<HostData>)> {
        self.hosts.iter().map(|(config, host)| (config, host))
    }

    #[must_use]
    pub fn host(&self, id: &HostId) -> Option<&Entity<HostData>> {
        self.hosts
            .iter()
            .find(|(config, _)| &config.id == id)
            .map(|(_, host)| host)
    }

    /// The host the current screen is on, else the last used, else the first.
    #[must_use]
    pub fn active_host(&self) -> Option<&Entity<HostData>> {
        self.screen
            .host()
            .and_then(|id| self.host(id))
            .or_else(|| self.files.hosts().ok()?.last.and_then(|id| self.host(&id)))
            .or_else(|| self.hosts.first().map(|(_, host)| host))
    }

    /// Saves a newly paired host and connects to it.
    pub fn add_host(&mut self, config: HostConfig, cx: &mut Context<Self>) -> Result<()> {
        self.connect(config.clone(), cx)?;
        let mut file = self.files.hosts()?;
        file.hosts.retain(|known| known.id != config.id);
        file.hosts.push(config.clone());
        file.last = Some(config.id);
        self.files.save_hosts(&file)?;
        cx.emit(CoreEvent::HostsChanged);
        cx.notify();
        Ok(())
    }

    pub fn remove_host(&mut self, id: &HostId, cx: &mut Context<Self>) -> Result<()> {
        self.hosts.retain(|(config, _)| &config.id != id);
        let mut file = self.files.hosts()?;
        file.hosts.retain(|known| &known.id != id);
        if file.last.as_ref() == Some(id) {
            file.last = None;
        }
        self.files.save_hosts(&file)?;
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
        if screen == self.screen {
            return;
        }
        if let Some(host) = screen.host()
            && let Ok(mut file) = self.files.hosts()
            && file.last.as_ref() != Some(host)
        {
            file.last = Some(host.clone());
            if let Err(error) = self.files.save_hosts(&file) {
                tracing::warn!("couldn't remember the last host: {error:#}");
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
            self.screen = screen;
            cx.emit(CoreEvent::ScreenChanged);
            cx.notify();
        }
    }

    /// Stores a device-local preference.
    pub fn set_setting(&mut self, key: &str, value: serde_json::Value, cx: &mut Context<Self>) {
        self.settings.insert(key.to_owned(), value);
        if let Err(error) = self.files.save_settings(&self.settings) {
            tracing::error!("couldn't save settings: {error:#}");
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
                identity: Identity {
                    certificate: String::new(),
                    key: String::new(),
                },
                settings: EditorSettings::new(),
                hosts: Vec::new(),
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
